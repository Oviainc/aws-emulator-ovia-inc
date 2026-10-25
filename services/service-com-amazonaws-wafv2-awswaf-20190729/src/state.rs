// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wafv2-awswaf-20190729 service.
//!
//! WAFv2 namespaces every resource by `Scope` (`CLOUDFRONT` | `REGIONAL`), so a
//! CLOUDFRONT web ACL named `x` and a REGIONAL one named `x` are two different
//! resources. Every map below is therefore keyed by `(Scope, …)`.
//!
//! An entry stores the MODELLED shape (`WebACL` / `IPSet`) the create call built,
//! not a flattened copy of its members, so a `Get` hands back exactly what was
//! stored — including the rule tree — and no member can be lost in translation
//! when the model grows one. The API bookkeeping that is not part of the shape
//! (its scope, its optimistic-lock token and its tags) sits beside it.

use std::collections::HashMap;
use std::sync::LazyLock;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    validate, ElementConstraintsTrait, LengthTrait, PatternTrait, Violation,
};

use crate::ops::types::{
    BlockAction, ChallengeAction, CountAction, IPSet, LabelSummary, ManagedProductDescriptor,
    OnSourceDDoSProtectionConfig, RegexPatternSet, Rule, RuleAction, RuleGroup, RuleSummary,
    Statement, Tag, WAFAssociatedItemException, WAFDuplicateItemException,
    WAFExpiredManagedRuleGroupVersionException, WAFFeatureNotIncludedInPricingPlanException,
    WAFInvalidOperationException, WAFInvalidParameterException, WAFInvalidPermissionPolicyException,
    WAFLimitsExceededException, WAFNonexistentItemException, WAFOptimisticLockException, WebACL,
};

/// The `Scope` values the model declares
/// (`smithy_query(query="shape",shape="Scope")` → `CLOUDFRONT`, `REGIONAL`).
pub const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";
/// See [`SCOPE_CLOUDFRONT`].
pub const SCOPE_REGIONAL: &str = "REGIONAL";

/// The region an ARN carries when the request names none: the emulator's default
/// endpoint region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `true` when `scope` is one of the two values the `Scope` enum declares.
pub fn is_known_scope(scope: &str) -> bool {
    matches!(scope, SCOPE_CLOUDFRONT | SCOPE_REGIONAL)
}

// ---------------------------------------------------------------------------
// The exception messages: ONE constant sentence per exception type.
// ---------------------------------------------------------------------------
//
// Every WAFv2 exception below models its detail as a lone `Message` (the limits
// error adds a `SourceType`), and the model documents ONE sentence for the SHAPE
// itself — `smithy_query(query="shape",shape="WAFNonexistentItemException")` is
// "WAF couldn’t perform the operation because your resource doesn't exist …", not
// a template with a slot for the resource that was named. The message is therefore
// a property of the exception TYPE, not of the call: WAF answers the same sentence
// for every request that trips that exception, and the body carries no name, id,
// ARN or scope taken from the request. A handler that needs to record WHICH
// resource tripped it writes that to the audit log ([`audit`]), never to the
// response.
//
// `WAFInvalidParameterException` is the one exception whose body does describe the
// call: the model gives it `Field`, `Parameter` and `Reason` members for exactly
// that ("The invalid parameter that resulted in the exception"), so its message
// stays request-derived. `WAFInternalErrorException` already carries a constant,
// request-independent sentence naming the internal fault it reports.

/// The sentence `WAFDuplicateItemException` carries, whatever the request named.
///
/// The wire wording WAF answers with, as the recorded wafv2 responses show it; the
/// model documents the same single sentence for the shape (see the note above).
pub const DUPLICATE_ITEM_MESSAGE: &str = "AWS WAF couldn’t perform the operation \
     because some resource in your request is a duplicate of an existing one.";

/// The sentence `WAFOptimisticLockException` carries. See
/// [`DUPLICATE_ITEM_MESSAGE`] for where the wording comes from.
pub const OPTIMISTIC_LOCK_MESSAGE: &str = "AWS WAF couldn’t save your changes \
     because someone changed the resource after you started to edit it. Reapply \
     your changes.";

/// The sentence `WAFNonexistentItemException` carries. See
/// [`DUPLICATE_ITEM_MESSAGE`].
pub const NONEXISTENT_ITEM_MESSAGE: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// The sentence `WAFAssociatedItemException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFAssociatedItemException")`, its
/// wrapping whitespace flattened), which is what pins it as constant per exception
/// type.
pub const ASSOCIATED_ITEM_MESSAGE: &str = "WAF couldn’t perform the operation \
     because your resource is being used by another resource or it’s associated \
     with another resource.";

/// The sentence `WAFLimitsExceededException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFLimitsExceededException")`),
/// link text flattened, which names no particular quota or request.
pub const LIMITS_EXCEEDED_MESSAGE: &str = "WAF couldn’t perform the operation \
     because you exceeded your resource limit. For example, the maximum number of \
     WebACL objects that you can create for an Amazon Web Services account. For \
     more information, see WAF quotas in the WAF Developer Guide.";

/// `WAFDuplicateItemException` as WAF returns it: nothing but
/// [`DUPLICATE_ITEM_MESSAGE`].
pub fn duplicate_item_exception() -> WAFDuplicateItemException {
    WAFDuplicateItemException {
        message: Some(DUPLICATE_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFOptimisticLockException` as WAF returns it: nothing but
/// [`OPTIMISTIC_LOCK_MESSAGE`].
pub fn optimistic_lock_exception() -> WAFOptimisticLockException {
    WAFOptimisticLockException {
        message: Some(OPTIMISTIC_LOCK_MESSAGE.to_string()),
    }
}

/// `WAFNonexistentItemException` as WAF returns it: nothing but
/// [`NONEXISTENT_ITEM_MESSAGE`].
pub fn nonexistent_item_exception() -> WAFNonexistentItemException {
    WAFNonexistentItemException {
        message: Some(NONEXISTENT_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFAssociatedItemException` as WAF returns it: nothing but
/// [`ASSOCIATED_ITEM_MESSAGE`].
pub fn associated_item_exception() -> WAFAssociatedItemException {
    WAFAssociatedItemException {
        message: Some(ASSOCIATED_ITEM_MESSAGE.to_string()),
    }
}

/// `WAFLimitsExceededException` as WAF returns it: [`LIMITS_EXCEEDED_MESSAGE`] and
/// no `SourceType`, which the model leaves optional and which this emulator has no
/// source value to report.
pub fn limits_exceeded_exception() -> WAFLimitsExceededException {
    WAFLimitsExceededException {
        message: Some(LIMITS_EXCEEDED_MESSAGE.to_string()),
        source_type: None,
    }
}

/// The sentence `WAFFeatureNotIncludedInPricingPlanException` carries: the shape's
/// own documented sentence
/// (`smithy_query(query="shape",shape="WAFFeatureNotIncludedInPricingPlanException")`).
/// See [`DUPLICATE_ITEM_MESSAGE`] for why the wording is a property of the
/// exception type rather than of the call.
pub const FEATURE_NOT_INCLUDED_IN_PRICING_PLAN_MESSAGE: &str = "The operation failed \
     because the specified WAF feature isn’t supported by the CloudFront pricing \
     plan associated with the web ACL.";

/// `WAFFeatureNotIncludedInPricingPlanException` as WAF returns it:
/// [`FEATURE_NOT_INCLUDED_IN_PRICING_PLAN_MESSAGE`] and no `DisallowedFeatures`.
///
/// The model leaves `DisallowedFeatures` optional and gives no vocabulary for
/// either of its members (`PricingPlanFeatureName` and `RequiredPricingPlanName`
/// are free strings, with no enum and no documented value list), so this emulator
/// reports no feature/plan names rather than inventing two.
pub fn feature_not_included_in_pricing_plan_exception()
-> WAFFeatureNotIncludedInPricingPlanException {
    WAFFeatureNotIncludedInPricingPlanException {
        message: Some(FEATURE_NOT_INCLUDED_IN_PRICING_PLAN_MESSAGE.to_string()),
        disallowed_features: None,
    }
}

/// The sentence `WAFInvalidOperationException` carries: the shape's own documented
/// sentence (`smithy_query(query="shape",shape="WAFInvalidOperationException")`),
/// its wrapping whitespace flattened. See [`DUPLICATE_ITEM_MESSAGE`] for why the
/// wording is a property of the exception type rather than of the call.
pub const INVALID_OPERATION_MESSAGE: &str = "The operation isn't valid.";

/// `WAFInvalidOperationException` as WAF returns it: nothing but
/// [`INVALID_OPERATION_MESSAGE`].
///
/// This is the refusal for a call an operation's own preconditions forbid on the
/// resource it names — the request parsed, the resource exists, and the operation
/// still may not be applied to it.
pub fn invalid_operation_exception() -> WAFInvalidOperationException {
    WAFInvalidOperationException {
        message: Some(INVALID_OPERATION_MESSAGE.to_string()),
    }
}

/// `WAFExpiredManagedRuleGroupVersionException` as WAF returns it: the sentence
/// naming the version that has expired, the managed rule group it is a version of
/// and the vendor that published it.
///
/// The one refusal of this service whose sentence is a TEMPLATE rather than a
/// constant: WAF words this one with the three values the call named — "You're
/// trying to use an expired version (`<VersionName>`) of `<RuleGroupName>` by
/// `<VendorName>`." — so its fixed text is reproduced exactly and its variable
/// parts come from the request and the published catalogue. Every other exception
/// type of this slice carries the one sentence its type always carries (see
/// [`DUPLICATE_ITEM_MESSAGE`]).
pub fn expired_managed_rule_group_version_exception(
    vendor_name: &str,
    name: &str,
    version: &str,
) -> WAFExpiredManagedRuleGroupVersionException {
    WAFExpiredManagedRuleGroupVersionException {
        message: Some(format!(
            "You're trying to use an expired version ({version}) of {name} by {vendor_name}."
        )),
    }
}

/// The sentence `WAFInvalidPermissionPolicyException` carries: the wire wording WAF
/// answers with, as the recorded wafv2 responses show it.
///
/// A shape's documentation prose is a description of the error, NOT the sentence the
/// service puts on the wire for it; where the two differ the recorded wording is the
/// one callers see, so this constant carries that and not the first sentence of the
/// shape's `documentation` trait. See [`DUPLICATE_ITEM_MESSAGE`] for why the wording
/// is a property of the exception type rather than of the call: the restrictions that
/// shape's documentation lists are what every request is judged against, not a detail
/// of one call, so the body names neither the policy that was rejected nor which
/// restriction it broke.
pub const INVALID_PERMISSION_POLICY_MESSAGE: &str = "The policy specification doesn't \
     meet the requirements to be attached to an AWS WAF resource.";

/// `WAFInvalidPermissionPolicyException` as WAF returns it: nothing but
/// [`INVALID_PERMISSION_POLICY_MESSAGE`].
///
/// This is the ONE refusal for a permission policy outside the restrictions
/// `PutPermissionPolicy` documents, whichever of them the document breaks (see
/// [`permission_policy_is_valid`]).
pub fn invalid_permission_policy_exception() -> WAFInvalidPermissionPolicyException {
    WAFInvalidPermissionPolicyException {
        message: Some(INVALID_PERMISSION_POLICY_MESSAGE.to_string()),
    }
}

/// A WebACL as stored: the modelled shape plus the bookkeeping WAFv2 keeps beside
/// it.
#[derive(Debug, Clone)]
pub struct WebAclEntry {
    /// The scope the web ACL was created in.
    pub scope: String,
    /// Exactly what `GetWebACL` answers with.
    pub web_acl: WebACL,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// An IPSet as stored: the modelled shape plus its scope, lock token and tags.
#[derive(Debug, Clone)]
pub struct IpSetEntry {
    /// The scope the IP set was created in.
    pub scope: String,
    /// Exactly what `GetIPSet` answers with.
    pub ip_set: IPSet,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// A RegexPatternSet as stored: the modelled shape plus its scope, lock token and
/// tags.
#[derive(Debug, Clone)]
pub struct RegexPatternSetEntry {
    /// The scope the regex pattern set was created in.
    pub scope: String,
    /// Exactly what `GetRegexPatternSet` answers with.
    pub regex_pattern_set: RegexPatternSet,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
}

/// A RuleGroup as stored: the modelled shape plus its scope, lock token and tags.
#[derive(Debug, Clone)]
pub struct RuleGroupEntry {
    /// The scope the rule group was created in.
    pub scope: String,
    /// Exactly what `GetRuleGroup` answers with.
    pub rule_group: RuleGroup,
    /// The current optimistic-lock token, rotated on every successful mutation.
    pub lock_token: String,
    /// Tags given at create time (`ListTagsForResource` is a separate gap issue).
    pub tags: Vec<Tag>,
    /// The IAM policy `PutPermissionPolicy` attached to this rule group, as the
    /// caller wrote it, or `None` while none is attached.
    ///
    /// A rule group carries at most one: "you can attach only one policy with each
    /// PutPermissionPolicy request" and the policy is addressed by the rule group's
    /// own ARN, so attaching replaces whatever was attached. The document is stored
    /// as the request wrote it, byte for byte — a policy is opaque to WAF beyond the
    /// restrictions [`permission_policy_is_valid`] checks — so
    /// `GetPermissionPolicy` answers the caller's own text.
    pub permission_policy: Option<String>,
}

/// An API key as stored: the encrypted key value, the token domains it carries
/// and the bookkeeping WAFv2 reports beside them.
///
/// An API key is not addressed by a name and an id like the entities above — "the
/// generated, encrypted API key" IS its identity within a scope
/// (`DeleteAPIKey$APIKey`, `GetDecryptedAPIKey$APIKey`) — so the entry carries the
/// key value itself and the two members `APIKeySummary` reports beside it, plus
/// the pagination cursor this service orders and resumes a listing by (see
/// [`api_key_cursor`]).
#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    /// The scope the key was created in.
    pub scope: String,
    /// The encrypted key value, as the create call answered it.
    pub api_key: String,
    /// `APIKeySummary$TokenDomains`: "the token domains that are defined in this
    /// API key", in the order the create call gave them.
    pub token_domains: Vec<String>,
    /// `APIKeySummary$CreationTimestamp` ("the date and time that the key was
    /// created") as milliseconds since the Unix epoch, the resolution
    /// [`date_time_millis`] compares a modelled timestamp at.
    pub created_at_millis: i64,
    /// `APIKeySummary$Version`: "internal value used by WAF to manage the key"
    /// (`APIKeyVersion`, `range(min 0)`, default 0).
    pub version: i32,
    /// This entry's position in a listing: [`api_key_cursor`] of the creation
    /// sequence it was minted with.
    pub cursor: String,
}

/// The width an API-key pagination cursor is zero-padded to: the 20 digits
/// `u64::MAX` takes, so every cursor is the same length and comparing two as
/// strings orders them by creation.
const API_KEY_CURSOR_WIDTH: usize = 20;

/// The listing cursor of the `sequence`-th API key this service minted.
///
/// `ListAPIKeys`' marker is opaque to a client — the model gives `NextMarker` no
/// form beyond `length 1..256` and `pattern \S`, and an API key is up to 2048
/// characters long, so the key value itself cannot be one. This service mints the
/// cursor from the key's creation sequence instead, zero-padded to
/// [`API_KEY_CURSOR_WIDTH`] so the lexicographic comparison a resume does is the
/// numeric one, and so a listing enumerates a scope's keys in the order they were
/// created.
pub fn api_key_cursor(sequence: u64) -> String {
    format!("{sequence:0width$}", width = API_KEY_CURSOR_WIDTH)
}

/// How many characters a well-formed API key value carries: the 256 bits of the
/// digest [`new_api_key`] renders, two hex characters per byte.
const API_KEY_RENDERED_LENGTH: usize = 64;

/// A fresh API key value for a key defined in `scope` over `token_domains`.
///
/// `CreateAPIKey` answers "the generated, encrypted API key", an `APIKey`
/// (`@length(1..2048)`, `@pattern(\S)` —
/// `smithy_query(query="shape",shape="APIKey")`), and the model gives the value no
/// further form: a client never parses it, it copies it into its JavaScript
/// CAPTCHA integration and hands it back to this service. So the value this
/// service mints is opaque — a 256-bit payload rendered in hex (the `hex` crate,
/// as [`new_uuid`] renders its own bytes), [`API_KEY_RENDERED_LENGTH`]
/// characters, every one of them non-whitespace.
///
/// The bytes are a SHA-256 digest (the `sha2` crate, as [`new_uuid`] uses it) over
/// a fresh [`new_uuid`] and the key's own definition, so a key is unique and
/// unpredictable per call: two keys defined over the same domains in the same
/// scope are two different keys, as two separate mints should be. What the key
/// DECRYPTS to is never recovered from the value — the token domains and the
/// creation instant are read back out of the store
/// ([`ComAmazonawsWafv2Awswaf20190729State::api_key`]), which is the only place
/// they are kept.
pub fn new_api_key(scope: &str, token_domains: &[String]) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(new_uuid().as_bytes());
    hasher.update(scope.as_bytes());
    for domain in token_domains {
        // A separator no `TokenDomain` can contain (`^[\w\.\-/]+$`), so two
        // different domain lists cannot hash the same bytes.
        hasher.update([0_u8]);
        hasher.update(domain.as_bytes());
    }
    ::hex::encode(hasher.finalize())
}

/// `true` when `value` has the form this service mints an API key in:
/// [`API_KEY_RENDERED_LENGTH`] lowercase hex characters.
///
/// A key is "the generated, encrypted API key", so a value that does not even
/// decode to a payload of the right size is not a key WAF could decrypt, whatever
/// the store holds — which is the difference between a key that is not valid and a
/// well-formed key naming nothing stored.
pub fn is_api_key_shaped(value: &str) -> bool {
    value.len() == API_KEY_RENDERED_LENGTH
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// One request a web ACL evaluated, as WAF's bot visibility recorded it: the URI
/// path the request asked for and the bot that sent it.
///
/// WAF reports `GetTopPathStatisticsByTraffic` by aggregating the requests a web
/// ACL actually evaluated over the window the caller names. This emulator serves
/// the WAFv2 control plane only — no HTTP request is ever evaluated against a
/// stored web ACL here — so nothing appends to
/// [`ComAmazonawsWafv2Awswaf20190729State::bot_requests`] and a web ACL that has
/// evaluated nothing reports zero requests over every window. The aggregation is
/// still the whole answer: a web ACL whose log holds requests reports exactly
/// those, grouped by path and ordered by volume.
#[derive(Debug, Clone)]
pub struct BotRequestRecord {
    /// When WAF evaluated the request, in milliseconds since the Unix epoch: a
    /// `TimeWindow` selects the records that fall inside it.
    pub observed_at_millis: i64,
    /// The URI path the request asked for, which `PathStatistics$Path` groups by.
    pub uri_path: String,
    /// The bot that sent the request, as `BotStatistics$BotName` names it and the
    /// `BotName` filter selects it.
    pub bot_name: String,
    /// The category the bot belongs to, as the `BotCategory` filter selects it.
    pub bot_category: String,
    /// The organization operating the bot, as the `BotOrganization` filter selects
    /// it.
    pub bot_organization: String,
}

/// What an ARN in [`ComAmazonawsWafv2Awswaf20190729State::arn_index`] resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A web ACL, by `(Scope, Id)`.
    WebAcl {
        /// The scope the web ACL lives in.
        scope: String,
        /// The web ACL's id.
        id: String,
    },
    /// An IP set, by `(Scope, Id)`.
    IpSet {
        /// The scope the IP set lives in.
        scope: String,
        /// The IP set's id.
        id: String,
    },
    /// A regex pattern set, by `(Scope, Id)`.
    ///
    /// `CreateRegexPatternSet` indexes every set it mints under this variant, so a
    /// `regexpatternset` ARN resolves to the pattern set it names and to nothing
    /// else — the same stance the other variants take (see
    /// [`Self::RuleGroup`]).
    RegexPatternSet {
        /// The scope the regex pattern set lives in.
        scope: String,
        /// The regex pattern set's id.
        id: String,
    },
    /// A rule group, by `(Scope, Id)`.
    ///
    /// `CreateRuleGroup` indexes every rule group it mints under this variant, so a
    /// `rulegroup` ARN resolves to the rule group it names and to nothing else: the
    /// index resolves an ARN to its resource KIND as well as to its identity, so a
    /// reference of one kind is never mistaken for a resource of another. See
    /// [`ComAmazonawsWafv2Awswaf20190729State::holds_rule_group`].
    RuleGroup {
        /// The scope the rule group lives in.
        scope: String,
        /// The rule group's id.
        id: String,
    },
}

/// Per-service state for com-amazonaws-wafv2-awswaf-20190729. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWafv2Awswaf20190729State {
    /// Web ACLs by `(Scope, Id)`.
    pub web_acls: HashMap<(String, String), WebAclEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub web_acl_names: HashMap<(String, String), String>,
    /// IP sets by `(Scope, Id)`.
    pub ip_sets: HashMap<(String, String), IpSetEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub ip_set_names: HashMap<(String, String), String>,
    /// Regex pattern sets by `(Scope, Id)`.
    pub regex_pattern_sets: HashMap<(String, String), RegexPatternSetEntry>,
    /// `(Scope, Name)` → `Id`: duplicate-name detection and Name+Id lookup.
    pub regex_pattern_set_names: HashMap<(String, String), String>,
    /// Rule groups by `(Scope, Id)`.
    pub rule_groups: HashMap<(String, String), RuleGroupEntry>,
    /// `(Scope, Name)` to `Id`, for rule groups: duplicate-name detection and
    /// Name+Id lookup.
    pub rule_group_names: HashMap<(String, String), String>,
    /// API keys by `(Scope, APIKey)`: the key value is the key's identity within
    /// its scope. See [`ApiKeyEntry`].
    pub api_keys: HashMap<(String, String), ApiKeyEntry>,
    /// How many API keys this service has minted, over every scope: the creation
    /// sequence [`ApiKeyEntry::cursor`] is rendered from.
    pub api_keys_minted: u64,
    /// Every resource ARN this service minted, so a by-ARN lookup resolves from
    /// state instead of parsing the ARN back apart.
    pub arn_index: HashMap<String, ResourceRef>,
    /// The requests each web ACL has evaluated, by `(Scope, Id)`: the log
    /// `GetTopPathStatisticsByTraffic` aggregates. See [`BotRequestRecord`].
    pub bot_requests: HashMap<(String, String), Vec<BotRequestRecord>>,
    /// The live web ACL association of every protected resource: the resource's
    /// own ARN → the ARN of the web ACL that is in front of it.
    ///
    /// Keyed by the RESOURCE, because a protected resource has at most one web
    /// ACL in front of it — `GetWebACLForResource` answers "the web ACL that is
    /// associated with the resource", one shape and not a list — while one web ACL
    /// protects any number of resources, which is the direction
    /// `ListResourcesForWebACL` reports. A protected resource is NOT a wafv2
    /// resource: it belongs to another service (see [`PROTECTED_RESOURCE_FORMS`]),
    /// so it is held by ARN and is not in [`Self::arn_index`].
    pub web_acl_associations: HashMap<String, String>,
}

impl ComAmazonawsWafv2Awswaf20190729State {
    /// Store an API key under `scope`, with the next creation cursor, and answer
    /// the entry as it was stored — or `None` when the scope already holds
    /// [`MAX_API_KEYS_PER_SCOPE`] of them, which is the refusal a minting
    /// operation reports as its modelled `WAFLimitsExceededException`.
    ///
    /// A key value is the key's identity within its scope, so storing one the
    /// scope ALREADY holds is idempotent: the stored entry is answered unchanged,
    /// keeping its creation instant and its place in the listing order rather than
    /// being re-sequenced to the end.
    ///
    /// The cursor bookkeeping and the bound live here rather than in a handler so
    /// that every operation that mints a key gets a position in the listing order
    /// without a listing having to invent one, and so no caller can grow this map
    /// without limit.
    pub fn insert_api_key(
        &mut self,
        scope: &str,
        api_key: &str,
        token_domains: Vec<String>,
        created_at_millis: i64,
    ) -> Option<ApiKeyEntry> {
        let key = (scope.to_string(), api_key.to_string());
        if let Some(stored) = self.api_keys.get(&key) {
            return Some(stored.clone());
        }
        if self.api_keys_count_in_scope(scope) >= MAX_API_KEYS_PER_SCOPE {
            return None;
        }
        self.api_keys_minted = self.api_keys_minted.saturating_add(1);
        let entry = ApiKeyEntry {
            scope: scope.to_string(),
            api_key: api_key.to_string(),
            token_domains,
            created_at_millis,
            // `APIKeyVersion`'s own default: nothing here re-versions a stored
            // key, so a key keeps the version it was minted with.
            version: 0,
            cursor: api_key_cursor(self.api_keys_minted),
        };
        self.api_keys.insert(key, entry.clone());
        Some(entry)
    }

    /// The API key `api_key` stored under `scope`, or `None` when the scope holds
    /// no key with that value.
    ///
    /// A key value is the key's identity within its scope, and a scope is a
    /// namespace, so the key value of one scope names no key of the other.
    pub fn api_key(&self, scope: &str, api_key: &str) -> Option<&ApiKeyEntry> {
        self.api_keys.get(&(scope.to_string(), api_key.to_string()))
    }

    /// How many API keys this scope already holds. See
    /// [`MAX_API_KEYS_PER_SCOPE`].
    pub fn api_keys_count_in_scope(&self, scope: &str) -> usize {
        self.api_keys
            .values()
            .filter(|entry| entry.scope == scope)
            .count()
    }

    /// Every API key stored under `scope`, in the order they were created.
    ///
    /// A scope is a namespace, so a listing of one scope never reports a key of
    /// the other, and the order is the cursor order a marker resumes from (see
    /// [`api_key_cursor`]).
    pub fn api_keys_in_scope(&self, scope: &str) -> Vec<&ApiKeyEntry> {
        let mut entries: Vec<&ApiKeyEntry> = self
            .api_keys
            .values()
            .filter(|entry| entry.scope == scope)
            .collect();
        entries.sort_by(|left, right| left.cursor.cmp(&right.cursor));
        entries
    }

    /// The web ACL stored under `(scope, id)`, but only when it is also the one
    /// called `name`. WAFv2's Get/Update/Delete take Name, Scope AND Id together,
    /// and a triple that does not describe one stored resource names no resource
    /// at all.
    pub fn web_acl(&self, scope: &str, name: &str, id: &str) -> Option<&WebAclEntry> {
        let entry = self.web_acls.get(&(scope.to_string(), id.to_string()))?;
        (entry.web_acl.name == name).then_some(entry)
    }

    /// The requests recorded for the web ACL `(scope, id)`, in whatever order they
    /// were appended — the aggregation over them is order-independent. Empty for a
    /// web ACL that has evaluated none, which is every web ACL this emulator holds
    /// (see [`BotRequestRecord`]).
    pub fn bot_requests(&self, scope: &str, id: &str) -> &[BotRequestRecord] {
        self.bot_requests
            .get(&(scope.to_string(), id.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// [`Self::web_acl`], mutably.
    pub fn web_acl_mut(&mut self, scope: &str, name: &str, id: &str) -> Option<&mut WebAclEntry> {
        let entry = self.web_acls.get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.web_acl.name == name).then_some(entry)
    }

    /// The IP set stored under `(scope, id)` when it is also the one called
    /// `name`. See [`Self::web_acl`].
    pub fn ip_set(&self, scope: &str, name: &str, id: &str) -> Option<&IpSetEntry> {
        let entry = self.ip_sets.get(&(scope.to_string(), id.to_string()))?;
        (entry.ip_set.name == name).then_some(entry)
    }

    /// [`Self::ip_set`], mutably.
    pub fn ip_set_mut(&mut self, scope: &str, name: &str, id: &str) -> Option<&mut IpSetEntry> {
        let entry = self.ip_sets.get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.ip_set.name == name).then_some(entry)
    }

    /// Drop a web ACL and every index entry that pointed at it.
    pub fn remove_web_acl(&mut self, scope: &str, id: &str) -> Option<WebAclEntry> {
        let entry = self.web_acls.remove(&(scope.to_string(), id.to_string()))?;
        self.web_acl_names
            .remove(&(entry.scope.clone(), entry.web_acl.name.clone()));
        self.arn_index.remove(&entry.web_acl.arn);
        Some(entry)
    }

    /// Drop an IP set and every index entry that pointed at it.
    pub fn remove_ip_set(&mut self, scope: &str, id: &str) -> Option<IpSetEntry> {
        let entry = self.ip_sets.remove(&(scope.to_string(), id.to_string()))?;
        self.ip_set_names
            .remove(&(entry.scope.clone(), entry.ip_set.name.clone()));
        self.arn_index.remove(&entry.ip_set.arn);
        Some(entry)
    }

    /// Drop a regex pattern set and every index entry that pointed at it.
    ///
    /// `RegexPatternSet$Name` and `$ARN` are modelled as optional, so an entry
    /// whose stored shape carries neither leaves no index key behind to remove.
    pub fn remove_regex_pattern_set(
        &mut self,
        scope: &str,
        id: &str,
    ) -> Option<RegexPatternSetEntry> {
        let entry = self
            .regex_pattern_sets
            .remove(&(scope.to_string(), id.to_string()))?;
        if let Some(name) = entry.regex_pattern_set.name.clone() {
            self.regex_pattern_set_names
                .remove(&(entry.scope.clone(), name));
        }
        if let Some(arn) = entry.regex_pattern_set.arn.as_deref() {
            self.arn_index.remove(arn);
        }
        Some(entry)
    }

    /// How many web ACLs this scope already holds. See [`MAX_WEB_ACLS_PER_SCOPE`].
    pub fn web_acls_in_scope(&self, scope: &str) -> usize {
        self.web_acls
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// How many IP sets this scope already holds. See [`MAX_IP_SETS_PER_SCOPE`].
    pub fn ip_sets_in_scope(&self, scope: &str) -> usize {
        self.ip_sets
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// The regex pattern set stored under `(scope, id)` when it is also the one
    /// called `name`. See [`Self::web_acl`].
    pub fn regex_pattern_set(
        &self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&RegexPatternSetEntry> {
        let entry = self
            .regex_pattern_sets
            .get(&(scope.to_string(), id.to_string()))?;
        (entry.regex_pattern_set.name.as_deref() == Some(name)).then_some(entry)
    }

    /// [`Self::regex_pattern_set`], mutably.
    pub fn regex_pattern_set_mut(
        &mut self,
        scope: &str,
        name: &str,
        id: &str,
    ) -> Option<&mut RegexPatternSetEntry> {
        let entry = self
            .regex_pattern_sets
            .get_mut(&(scope.to_string(), id.to_string()))?;
        (entry.regex_pattern_set.name.as_deref() == Some(name)).then_some(entry)
    }

    /// How many regex pattern sets this scope already holds. See
    /// [`MAX_REGEX_PATTERN_SETS_PER_SCOPE`].
    pub fn regex_pattern_sets_in_scope(&self, scope: &str) -> usize {
        self.regex_pattern_sets
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// The rule group stored under `(scope, id)` when it is also the one called
    /// `name`. See [`Self::web_acl`].
    pub fn rule_group(&self, scope: &str, name: &str, id: &str) -> Option<&RuleGroupEntry> {
        let entry = self.rule_groups.get(&(scope.to_string(), id.to_string()))?;
        (entry.rule_group.name == name).then_some(entry)
    }

    /// How many rule groups this scope already holds. See
    /// [`MAX_RULE_GROUPS_PER_SCOPE`].
    pub fn rule_groups_in_scope(&self, scope: &str) -> usize {
        self.rule_groups
            .keys()
            .filter(|(stored_scope, _)| stored_scope == scope)
            .count()
    }

    /// Every rule group ARN this scope holds, with the group's declared
    /// `Capacity`.
    ///
    /// This is what prices a `RuleGroupReferenceStatement` in
    /// [`web_acl_capacity`]: `RuleGroup$Capacity` is "the web ACL capacity units
    /// (WCUs) required for this rule group", fixed at creation, so a web ACL rule
    /// that runs a rule group consumes exactly that group's declared capacity. It
    /// is read out of state BEFORE a save writes through it, so a handler can hold
    /// it while it borrows the entry it is about to update.
    pub fn rule_group_capacities(&self, scope: &str) -> HashMap<String, i64> {
        self.rule_groups
            .values()
            .filter(|entry| entry.scope == scope)
            .map(|entry| (entry.rule_group.arn.clone(), entry.rule_group.capacity))
            .collect()
    }

    /// Why a reference in `rules` cannot be resolved, if any cannot.
    ///
    /// WAF resolves a rule statement's reference when the rule is SAVED: a
    /// reference statement names another entity ("the Amazon Resource Name (ARN) of
    /// the IPSet that this statement references", `IPSetReferenceStatement$ARN`),
    /// and a save naming an entity that does not exist is refused with the modelled
    /// `WAFNonexistentItemException` ("WAF couldn’t perform the operation because
    /// your resource doesn't exist") rather than stored for a later `Get` to hand
    /// back as though the reference were live. This is the ingress counterpart of
    /// [`Self::web_acl_referencing`]: the same reference keeps an entity from
    /// being deleted and keeps a rule that names a missing one from being stored.
    ///
    /// Each of the model's four reference-bearing statements is resolved against
    /// the entities that KIND of reference names. An `IPSetReferenceStatement`, a
    /// `RuleGroupReferenceStatement` and a `RegexPatternSetReferenceStatement` name
    /// resources of this account, so they resolve
    /// through [`Self::arn_index`], each only to a resource of its own kind in the
    /// SAME scope, because a scope is a namespace. A `ManagedRuleGroupStatement`
    /// names no resource of this account at all — it names a group its VENDOR
    /// publishes, identified by `(VendorName, Name)` — so it resolves against
    /// [`MANAGED_RULE_GROUP_CATALOGUE`], the catalogue this service publishes, and a
    /// pair the catalogue does not list names nothing that exists.
    /// The `Some` it answers is the DIAGNOSTIC for the refusal's
    /// audit record — the response itself carries [`NONEXISTENT_ITEM_MESSAGE`], the
    /// constant sentence that exception type always carries. That is also what keeps
    /// [`web_acl_capacity`] from pricing a statement whose cost it cannot know: a
    /// stored rule list can only carry a reference to a rule group this service
    /// holds, whose declared `Capacity` is exactly that statement's cost, or to a
    /// catalogued managed rule group, whose published capacity is its cost.
    ///
    /// Walked with an explicit worklist, like [`validate_statement_nesting`]: the
    /// tree is supplied by the request.
    pub fn unresolvable_rule_reference(&self, scope: &str, rules: &[Rule]) -> Option<String> {
        let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
        while let Some(statement) = pending.pop() {
            if let Some(reference) = statement.ip_set_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::IpSet {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No IPSet with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::RuleGroup {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No RuleGroup with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(reference) = statement.regex_pattern_set_reference_statement.as_ref() {
                let resolved = matches!(
                    self.arn_index.get(&reference.arn),
                    Some(ResourceRef::RegexPatternSet {
                        scope: stored_scope,
                        ..
                    }) if stored_scope == scope
                );
                if !resolved {
                    return Some(format!(
                        "No RegexPatternSet with the ARN {} exists in scope {scope}",
                        reference.arn
                    ));
                }
            }
            if let Some(managed) = statement
                .managed_rule_group_statement
                .as_ref()
                .filter(|managed| {
                    managed_rule_group(&managed.vendor_name, &managed.name).is_none()
                })
            {
                return Some(format!(
                    "No managed rule group named {:?} from vendor {:?} is available",
                    managed.name, managed.vendor_name
                ));
            }
            pending.extend(nested_statements(statement));
        }
        None
    }

    /// `true` when `arn` names a rule group this service holds.
    ///
    /// WAF resolves a published version's `AssociatedRuleGroupArn` against the
    /// vendor's own rule groups, so this is what
    /// `PutManagedRuleSetVersions` consults before it judges anything else about
    /// the request. The ARN must be a well-formed wafv2 ARN whose resource kind is
    /// `rulegroup` AND be indexed as one: an ARN of another kind, or one naming a
    /// rule group that was never created, resolves to nothing.
    ///
    /// A rule group is indexed under [`ResourceRef::RuleGroup`] by
    /// `CreateRuleGroup`, so this answers `true` for a rule group this service
    /// holds and `false` for an ARN of another kind or one naming a rule group that
    /// was never created. That is the same stance
    /// [`Self::unresolvable_rule_reference`] takes for a
    /// `RuleGroupReferenceStatement`: a reference WAF cannot resolve is refused
    /// rather than stored as though it were live.
    pub fn holds_rule_group(&self, arn: &str) -> bool {
        arn_resource_kind(arn) == Some(RULE_GROUP_ARN_KIND)
            && matches!(self.arn_index.get(arn), Some(ResourceRef::RuleGroup { .. }))
    }

    /// The rule group `arn` names, or `None` when it names no rule group this
    /// service holds.
    ///
    /// The by-ARN counterpart of [`Self::rule_group`], resolving through
    /// [`Self::arn_index`] exactly as [`Self::web_acl_by_arn`] does, so an ARN of
    /// another kind and an ARN that was never minted here both name nothing — and so
    /// does a rule group ARN carrying another Region, since the index holds each
    /// resource under the one ARN it was minted with.
    pub fn rule_group_by_arn(&self, arn: &str) -> Option<&RuleGroupEntry> {
        match self.arn_index.get(arn)? {
            ResourceRef::RuleGroup { scope, id } => {
                self.rule_groups.get(&(scope.clone(), id.clone()))
            }
            ResourceRef::WebAcl { .. }
            | ResourceRef::IpSet { .. }
            | ResourceRef::RegexPatternSet { .. } => None,
        }
    }

    /// [`Self::rule_group_by_arn`], for a caller that writes to the entry.
    pub fn rule_group_by_arn_mut(&mut self, arn: &str) -> Option<&mut RuleGroupEntry> {
        let (scope, id) = match self.arn_index.get(arn)? {
            ResourceRef::RuleGroup { scope, id } => (scope.clone(), id.clone()),
            ResourceRef::WebAcl { .. }
            | ResourceRef::IpSet { .. }
            | ResourceRef::RegexPatternSet { .. } => return None,
        };
        self.rule_groups.get_mut(&(scope, id))
    }

    /// The live web ACL whose rule tree references `arn`, if any.
    ///
    /// A resource that a web ACL still references cannot be deleted, so this is
    /// what a delete consults, whatever KIND of resource is being deleted: an ARN
    /// identifies one resource of one kind, so the walk asks only whether any
    /// reference statement names this ARN. It walks the whole tree, because a
    /// reference statement can sit inside `AndStatement`, `OrStatement`,
    /// `NotStatement` or a rate-based/managed-rule-group scope-down statement.
    pub fn web_acl_referencing(&self, arn: &str) -> Option<&WebAclEntry> {
        self.web_acls.values().find(|entry| {
            entry
                .web_acl
                .rules
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|rule| {
                    statement_references_arn(&rule.statement, arn, MAX_STATEMENT_NESTING_DEPTH)
                })
        })
    }

    /// The web ACL `arn` names, or `None` when it names no web ACL this service
    /// holds.
    ///
    /// The index resolves an ARN to its resource KIND as well as to its identity,
    /// so an ARN of another kind — an IP set, a rule group, a regex pattern set —
    /// names no web ACL, and neither does an ARN that was never minted here. That
    /// is the same stance [`Self::holds_rule_group`] takes.
    pub fn web_acl_by_arn(&self, arn: &str) -> Option<&WebAclEntry> {
        match self.arn_index.get(arn)? {
            ResourceRef::WebAcl { scope, id } => self.web_acls.get(&(scope.clone(), id.clone())),
            ResourceRef::IpSet { .. }
            | ResourceRef::RegexPatternSet { .. }
            | ResourceRef::RuleGroup { .. } => None,
        }
    }

    /// Put the web ACL `web_acl_arn` in front of the resource `resource_arn`, and
    /// answer whether the binding was stored.
    ///
    /// A resource has at most ONE web ACL in front of it, so binding a resource
    /// that is already protected REPLACES its binding rather than adding a second
    /// one, and re-binding the resource to the web ACL it already carries changes
    /// nothing. `false` means the web ACL already protects
    /// [`MAX_PROTECTED_RESOURCES_PER_WEB_ACL`] resources, which is the refusal a
    /// binding operation reports as its modelled `WAFLimitsExceededException`.
    pub fn associate_web_acl(&mut self, resource_arn: &str, web_acl_arn: &str) -> bool {
        let already_bound =
            self.web_acl_associations.get(resource_arn).map(String::as_str) == Some(web_acl_arn);
        if !already_bound
            && self.protected_resources(web_acl_arn).len() >= MAX_PROTECTED_RESOURCES_PER_WEB_ACL
        {
            return false;
        }
        self.web_acl_associations
            .insert(resource_arn.to_string(), web_acl_arn.to_string());
        true
    }

    /// Every resource ARN the web ACL `web_acl_arn` is in front of, in ARN order
    /// so one listing of one web ACL always reports them the same way round.
    pub fn protected_resources(&self, web_acl_arn: &str) -> Vec<&str> {
        let mut arns: Vec<&str> = self
            .web_acl_associations
            .iter()
            .filter(|(_, bound)| bound.as_str() == web_acl_arn)
            .map(|(resource, _)| resource.as_str())
            .collect();
        arns.sort_unstable();
        arns
    }

    /// The web ACL in front of `resource_arn`, or `None` when the resource carries
    /// no association.
    pub fn web_acl_for_resource(&self, resource_arn: &str) -> Option<&WebAclEntry> {
        let web_acl_arn = self.web_acl_associations.get(resource_arn)?;
        self.web_acl_by_arn(web_acl_arn)
    }

    /// Take the web ACL away from `resource_arn`, and answer whether one was there
    /// to take away.
    pub fn disassociate_web_acl(&mut self, resource_arn: &str) -> bool {
        self.web_acl_associations.remove(resource_arn).is_some()
    }
}

/// The deepest a rule's statement tree may nest.
///
/// Statements nest: `AndStatement` and `OrStatement` over a list, `NotStatement`
/// over one, and the scope-down statement of a rate-based or managed-rule-group
/// statement. WAF publishes a maximum nesting depth for rule statements in its
/// quota list, and the tree comes straight off the wire, so the bound is enforced
/// ON INGRESS by [`validate_statement_nesting`] — before a tree is stored, before
/// its capacity is computed, and so before any later walk sees it.
pub const MAX_STATEMENT_NESTING_DEPTH: u32 = 10;

/// The most web ACLs this service stores per scope: WAF's published quota is 100
/// web ACLs per Region per scope, and reaching it answers the modelled
/// `WAFLimitsExceededException` the way the real service does — which is also what
/// keeps the store from growing without limit.
pub const MAX_WEB_ACLS_PER_SCOPE: usize = 100;

/// The most IP sets this service stores per scope; see [`MAX_WEB_ACLS_PER_SCOPE`].
/// WAF's published quota is 100 IP sets per Region per scope.
pub const MAX_IP_SETS_PER_SCOPE: usize = 100;

/// The most rule groups this service stores per scope; see
/// [`MAX_WEB_ACLS_PER_SCOPE`]. WAF's published quota is 100 rule groups per Region
/// per scope.
pub const MAX_RULE_GROUPS_PER_SCOPE: usize = 100;

/// The most resources one web ACL is put in front of before a binding is refused
/// with the modelled `WAFLimitsExceededException`.
///
/// Unlike [`MAX_WEB_ACLS_PER_SCOPE`] this is NOT a quota WAF publishes: it is this
/// emulator's own bound on
/// [`ComAmazonawsWafv2Awswaf20190729State::web_acl_associations`], whose keys are
/// resource ARNs the caller supplies, so that no caller can grow that map without
/// limit. It is the same size as the other per-scope bounds, and a web ACL at it
/// can still be re-bound to a resource it already protects.
pub const MAX_PROTECTED_RESOURCES_PER_WEB_ACL: usize = 100;

/// The most regex pattern sets this service stores per scope before
/// `CreateRegexPatternSet` answers its modelled `WAFLimitsExceededException`.
///
/// Unlike [`MAX_WEB_ACLS_PER_SCOPE`], [`MAX_IP_SETS_PER_SCOPE`] and
/// [`MAX_RULE_GROUPS_PER_SCOPE`], which carry WAF's published per-Region figure of
/// 100, no figure is cited here, so this bound is CHOSEN — on the same reasoning
/// the sibling waf-regional service states for its own pattern-set bound: every
/// created set is a stored record, so the number a caller can create has to be
/// bounded, and the bound is well above the handful of pattern sets a rule's
/// `RegexPatternSetReferenceStatement`s name while staying small enough for a test
/// to reach it. It is per scope because a scope is a namespace, like every other
/// bound in this module.
pub const MAX_REGEX_PATTERN_SETS_PER_SCOPE: usize = 10;

/// The most API keys this service stores per scope before a minting operation
/// answers its modelled `WAFLimitsExceededException`
/// ([`ComAmazonawsWafv2Awswaf20190729State::insert_api_key`] is what reports the
/// bound being reached).
///
/// Like [`MAX_REGEX_PATTERN_SETS_PER_SCOPE`], and unlike the three bounds above
/// it, no published figure is cited here, so this bound is CHOSEN, on the same
/// reasoning: every minted key is a stored record, so the number a caller can
/// mint has to be bounded, and the bound is well above the handful of keys a
/// client application's CAPTCHA integration defines while staying small enough
/// for a test to reach it. It is per scope because a scope is a namespace, like
/// every other bound in this module.
pub const MAX_API_KEYS_PER_SCOPE: usize = 10;

/// The most token domains ONE API key carries: "You can use a single key for up to
/// 5 domains" (`CreateAPIKey`'s own documentation,
/// `smithy_query(query="doc",shape="CreateAPIKey")`). A request naming more is
/// refused rather than stored, because a key that carries them could not be used
/// for all of them.
pub const MAX_TOKEN_DOMAINS_PER_API_KEY: usize = 5;

/// The most addresses one IP set holds: WAF's published quota is 10,000 IP
/// addresses or ranges per IP set. It bounds how large a single stored entry can
/// be, beside the serve path's own 16 MiB request-body cap.
pub const MAX_IP_SET_ADDRESSES: usize = 10_000;

// ---------------------------------------------------------------------------
// The managed rule group catalogue
// ---------------------------------------------------------------------------

/// The `VendorName` every rule group AWS itself publishes carries.
///
/// `ManagedRuleGroupStatement$VendorName` is "the name of the managed rule group
/// vendor. You use this, along with the rule group name, to identify a rule
/// group", and the groups in [`MANAGED_RULE_GROUP_CATALOGUE`] are the ones AWS
/// publishes, so they all carry this one vendor name.
pub const AWS_MANAGED_RULE_GROUP_VENDOR: &str = "AWS";

/// One managed rule group as its vendor publishes it: the pair that identifies it
/// and the capacity a web ACL spends to run it.
///
/// A managed rule group is NOT a resource of this account — nothing creates,
/// updates or deletes one — so it lives in a constant catalogue rather than in
/// [`ComAmazonawsWafv2Awswaf20190729State`], which holds only what this account
/// owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedRuleGroupDescriptor {
    /// The vendor that publishes the group
    /// (`ManagedRuleGroupStatement$VendorName`).
    pub vendor_name: &'static str,
    /// The group's name (`ManagedRuleGroupStatement$Name`).
    pub name: &'static str,
    /// The WCUs a web ACL spends on one rule that runs this group; see
    /// [`web_acl_capacity`].
    pub capacity: i64,
    /// The group's display name (`ManagedProductDescriptor$ProductTitle` is "the
    /// display name for the managed rule group"), as the WAF Developer Guide's
    /// rule-group list titles it beside its `Name`.
    pub product_title: &'static str,
    /// What the group protects against, in one sentence
    /// (`ManagedProductDescriptor$ProductDescription` is "a short description of
    /// the managed rule group"), from the same published list as
    /// [`Self::product_title`].
    pub product_description: &'static str,
    /// Whether the vendor offers this group in NAMED versions a web ACL can choose
    /// between (`ManagedProductDescriptor$IsVersioningSupported`), which is the same
    /// thing as the guide publishing a version history for it — so this is `true`
    /// for exactly the entries [`versioned`] builds (see [`BOT_CONTROL_VERSIONS`])
    /// and `false` for the ones whose vendor keeps a single version on offer
    /// ([`CURRENT_VERSION_ONLY`]) and retires the one it supersedes.
    pub is_versioning_supported: bool,
    /// Whether the group provides "an advanced set of protections, such as the
    /// the Amazon Web Services Managed Rules rule groups that are used for WAF
    /// intelligent threat mitigation"
    /// (`ManagedProductDescriptor$IsAdvancedManagedRuleSet`): the
    /// intelligent-threat-mitigation groups, which are the versioned ones.
    pub is_advanced_managed_rule_set: bool,
    /// The versions of this group its vendor offers, OLDEST first.
    ///
    /// "Customers can retrieve the available versions for a managed rule group by
    /// calling `ListAvailableManagedRuleGroupVersions`"
    /// (`smithy_query(query="doc",shape="PutManagedRuleSetVersions")`), and the
    /// versions on offer are the VENDOR's, not anything this account holds — so
    /// they sit here, beside the capacity, rather than in the state struct. Every
    /// catalogued group offers at least its initial version (see
    /// [`MANAGED_RULE_GROUP_CATALOGUE`]).
    pub versions: &'static [ManagedRuleGroupRelease],
    /// The versions of this group its vendor has RETIRED, oldest first: ones it
    /// published and has since stopped offering.
    ///
    /// A version is offered for a while and then expires:
    /// `DescribeManagedRuleGroup$VersionName` is "the version of the rule group.
    /// You can only use a version that is not scheduled for expiration", and a
    /// request naming one that has expired is answered with
    /// `WAFExpiredManagedRuleGroupVersionException` — "you can retrieve the
    /// available versions for the managed rule group by calling
    /// `ListAvailableManagedRuleGroupVersions`" (that error's own doc), which
    /// lists [`Self::versions`] alone. So an expired version is pruned from what
    /// the vendor OFFERS while staying KNOWN to the describe: a request naming one
    /// is refused as expired rather than as a version nobody published, which is
    /// what tells a version-pinning caller to roll forward. A group whose vendor
    /// has retired none carries an empty list.
    pub expired_versions: &'static [ManagedRuleGroupRelease],
    /// The rules this group runs, in the order its vendor lists them.
    ///
    /// `DescribeManagedRuleGroup` "provides high-level information for a managed
    /// rule group, including descriptions of the rules" (the operation's own doc),
    /// and `RuleSummary` is "high-level information about a Rule, returned by
    /// operations like DescribeManagedRuleGroup" — a name and the action the rule
    /// takes. Like [`Self::versions`] those are the VENDOR's, not anything this
    /// account holds: nothing creates, changes or removes a rule of a published
    /// group, so they sit in the catalogue beside the capacity.
    pub rules: &'static [PublishedManagedRule],
}

impl ManagedRuleGroupDescriptor {
    /// The version of this group that is currently the default: the NEWEST version
    /// its vendor offers.
    ///
    /// `ListAvailableManagedRuleGroupVersionsResponse$CurrentDefaultVersion` is
    /// "the name of the version that's currently set as the default", and
    /// `DescribeManagedRuleGroup$VersionName` says WAF "uses the vendor's default
    /// version" when a request names none. The catalogue keeps each group's
    /// versions in release order, so its default is the last of them: a group gains
    /// a default by being versioned, and gains a new one when a newer version is
    /// published, with no second field to fall out of step with the list.
    pub fn current_default_version(&self) -> Option<&'static str> {
        self.versions.last().map(|release| release.name)
    }

    /// This published group as the managed PRODUCT a describe reports it.
    ///
    /// A managed product IS a published managed rule group —
    /// `DescribeAllManagedProducts` "provides high-level information for the Amazon
    /// Web Services Managed Rules rule groups and Amazon Web Services Marketplace
    /// managed rule groups" (the operation's own doc) — so a descriptor is built
    /// from this catalogue entry rather than from a second list that could fall
    /// out of step with it: the `(VendorName, ManagedRuleSetName)` pair that
    /// identifies the group, its title and description, and its two flags.
    ///
    /// `ProductId` is "a unique identifier for the rule group", and a
    /// vendor-published group is identified by its name within its vendor, so the
    /// name is that identifier. `ProductLink` is left off: it is "for Amazon Web
    /// Services Marketplace managed rule groups only", and this catalogue
    /// publishes no marketplace group. `SnsTopicArn` is the topic "used to provide
    /// notification of changes to the managed rule group": one topic per group, in
    /// the region the group's `scope` is reached through, minted through the
    /// emulator's own ARN builder like every other ARN this service answers.
    pub fn product(&self, scope: &str, request_region: &str) -> ManagedProductDescriptor {
        ManagedProductDescriptor {
            vendor_name: Some(self.vendor_name.to_string()),
            managed_rule_set_name: Some(self.name.to_string()),
            product_id: Some(self.name.to_string()),
            product_link: None,
            product_title: Some(self.product_title.to_string()),
            product_description: Some(self.product_description.to_string()),
            sns_topic_arn: Some(self.sns_topic_arn(scope, request_region)),
            is_versioning_supported: self.is_versioning_supported,
            is_advanced_managed_rule_set: self.is_advanced_managed_rule_set,
        }
    }

    /// The ARN of the notification topic for this group: the topic "used to provide
    /// notification of changes to the managed rule group"
    /// (`ManagedProductDescriptor$SnsTopicArn`, and
    /// `DescribeManagedRuleGroupResponse$SnsTopicArn` documents the same thing), so
    /// one topic per group and the SAME ARN whichever call reports it.
    ///
    /// Minted through the emulator's own ARN builder, in the region the group's
    /// `scope` is reached through, like every other ARN this service answers.
    pub fn sns_topic_arn(&self, scope: &str, request_region: &str) -> String {
        ::engine::emulator_arn("sns", arn_region(scope, request_region), self.name)
    }

    /// The version of this group `requested` names, or the vendor's default when a
    /// request names none — and `None` when the vendor offers no such version.
    ///
    /// `DescribeManagedRuleGroup$VersionName` is "the version of the rule group…
    /// If you don't provide this, WAF uses the vendor's default version", so a
    /// request naming nothing describes [`Self::current_default_version`], and the
    /// versions a request may name are exactly the ones
    /// `ListAvailableManagedRuleGroupVersions` lists for the group — so a version
    /// its vendor has retired is not one of them, and is told apart from a version
    /// nobody published by [`Self::version_state`].
    pub fn offered_version(&self, requested: Option<&str>) -> Option<&'static str> {
        match requested {
            None => self.current_default_version(),
            Some(requested) => self
                .versions
                .iter()
                .find(|release| release.name == requested)
                .map(|release| release.name),
        }
    }

    /// What this group's vendor publishes about the version `requested` names:
    /// whether it is on offer, published and since expired, or nothing the vendor
    /// ever published.
    ///
    /// Those are the three states a version name can be in for a published group,
    /// and the model gives each its own answer: an offered one is described and can
    /// be pinned, an expired one is the modelled
    /// `WAFExpiredManagedRuleGroupVersionException` ("the specified version for the
    /// managed rule group has expired"), and one no vendor published is neither.
    /// Matched exactly, like the `(VendorName, Name)` pair itself: a
    /// `VersionKeyString` is the vendor's own name for the version.
    pub fn version_state(&self, requested: &str) -> ManagedVersionState {
        if self.versions.iter().any(|release| release.name == requested) {
            ManagedVersionState::Offered
        } else if self
            .expired_versions
            .iter()
            .any(|release| release.name == requested)
        {
            ManagedVersionState::Expired
        } else {
            ManagedVersionState::Unknown
        }
    }

    /// This group's rules as a describe reports them: a name and the action the
    /// vendor's rule takes (see [`Self::rules`]).
    pub fn rule_summaries(&self) -> Vec<RuleSummary> {
        self.rules
            .iter()
            .map(|rule| RuleSummary {
                name: Some(rule.name.to_string()),
                action: Some(rule.action.rule_action()),
            })
            .collect()
    }

    /// The labels this group's rules ADD to matching requests, deduplicated and in
    /// a stable order — the managed-group counterpart of
    /// [`rule_group_available_labels`], sharing its [`label_summaries`]
    /// projection, and the same reading of `AvailableLabels`: each entry is the
    /// label as the rule specifies it ([`PublishedManagedRule::label`]), with the
    /// prefix every one of them acquires at match time reported separately as
    /// [`Self::managed_label_namespace`].
    ///
    /// Only the labels this catalogue CITES for a rule are reported: a vendor
    /// publishes its label texts separately from its rule list, so a group whose
    /// labels this catalogue cannot cite reports none rather than a text nobody
    /// published (see [`PublishedManagedRule::published_label`]).
    pub fn available_labels(&self) -> Vec<LabelSummary> {
        label_summaries(self.rules.iter().filter_map(|rule| rule.label()))
    }

    /// `awswaf:managed:<vendor>:<rule group name>:` — the prefix every label added
    /// by a rule in this group carries.
    ///
    /// `DescribeManagedRuleGroupResponse$LabelNamespace` states the syntax:
    /// "The syntax for the label namespace prefix for a managed rule group is the
    /// following: `awswaf:managed:<vendor>:<rule group name>`:". A customer-owned
    /// entity's namespace is built the same way from its own identity, by
    /// [`label_namespace`]; a managed group is identified by its vendor and name
    /// rather than by this account, which is the only difference.
    pub fn managed_label_namespace(&self) -> String {
        format!("awswaf:managed:{}:{}:", self.vendor_name, self.name)
    }
}

/// The state one version name is in for a published managed rule group, as its
/// vendor publishes it.
///
/// Three states, because the model answers each differently: a version on offer is
/// described and can be pinned, an expired one is the modelled
/// `WAFExpiredManagedRuleGroupVersionException`, and a name no version of the group
/// ever carried is neither of those. See
/// [`ManagedRuleGroupDescriptor::version_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedVersionState {
    /// The vendor OFFERS this version: it is one of
    /// [`ManagedRuleGroupDescriptor::versions`], so
    /// `ListAvailableManagedRuleGroupVersions` lists it, a describe reports it and
    /// a `ManagedRuleGroupStatement` may pin it.
    Offered,
    /// The vendor published this version and has since retired it: it is one of
    /// [`ManagedRuleGroupDescriptor::expired_versions`], so it is no longer listed
    /// and every call that resolves it is refused as expired.
    Expired,
    /// No version of this group ever carried this name.
    Unknown,
}

/// One rule of a published managed rule group: its name and the action its vendor
/// has it take.
///
/// Flat and `Copy` like [`ManagedRuleGroupRelease`], because it is published data
/// rather than stored state: a caller cannot add, change or remove one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedManagedRule {
    /// The rule's name (`RuleSummary$Name`, an `EntityName`: `length 1..128`,
    /// pattern `^[\w\-]+$`), as the WAF Developer Guide's rule list for the group
    /// publishes it.
    pub name: &'static str,
    /// The action the vendor's rule takes on a request it matches
    /// (`RuleSummary$Action`), as the same list publishes it.
    pub action: PublishedRuleAction,
    /// The label this rule adds to a request it matches, where this catalogue
    /// cites the published text; `None` where it cannot.
    ///
    /// A vendor publishes its label texts separately from its rule list, and a
    /// label may read nothing like the rule that adds it, so a label is cited
    /// here only where the published text is known — never derived from the
    /// rule's name, which would report a text nobody published.
    pub published_label: Option<&'static str>,
}

impl PublishedManagedRule {
    /// The label this rule adds to a request it matches, as the rule specifies
    /// it, or `None` where this catalogue cites none for it.
    ///
    /// `LabelSummary` reports "the label as the rule specified it", and a fully
    /// qualified label "is made up of the label namespace from the rule group …
    /// and the label from the rule, separated by a colon"
    /// (`DescribeManagedRuleGroupResponse$LabelNamespace`) — so a cited label
    /// under [`ManagedRuleGroupDescriptor::managed_label_namespace`] is what a
    /// matching request carries. Stable per rule and per call: it is
    /// [`Self::published_label`] and nothing else.
    pub fn label(&self) -> Option<&'static str> {
        self.published_label
    }
}

/// The action a published managed rule takes on a request it matches.
///
/// Only the actions the published rule lists use: `RuleAction` models six, but a
/// vendor's rule list states `Block`, `Count` or `Challenge` for its rules, and a
/// variant this catalogue cannot cite a rule for would be an action nobody
/// publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedRuleAction {
    /// Block the request (`RuleAction$Block`).
    Block,
    /// Count the request (`RuleAction$Count`).
    Count,
    /// Run a silent challenge against the request (`RuleAction$Challenge`).
    Challenge,
}

impl PublishedRuleAction {
    /// This action as the modelled `RuleAction`: the one member the action names,
    /// with no custom response or request handling, which a vendor's own rule
    /// carries none of.
    pub fn rule_action(self) -> RuleAction {
        match self {
            Self::Block => RuleAction {
                block: Some(BlockAction::default()),
                ..RuleAction::default()
            },
            Self::Count => RuleAction {
                count: Some(CountAction::default()),
                ..RuleAction::default()
            },
            Self::Challenge => RuleAction {
                challenge: Some(ChallengeAction::default()),
                ..RuleAction::default()
            },
        }
    }
}

/// One version of a managed rule group as its vendor offers it.
///
/// Flat and `Copy`, like [`ManagedRuleGroupDescriptor`], because it is published
/// data rather than stored state: a caller cannot add, change or remove one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedRuleGroupRelease {
    /// The version name, which is what a `ManagedRuleGroupStatement$Version` pins
    /// (`ManagedRuleGroupVersion$Name`, a `VersionKeyString`: `length 1..64`,
    /// pattern `^[\w#:\.\-/]+$`).
    pub name: &'static str,
    /// When the vendor last updated this version's information
    /// (`ManagedRuleGroupVersion$LastUpdateTimestamp`), as an RFC 3339 instant in
    /// UTC — the form [`::aws_smithy_types::DateTime::from_str`] reads with
    /// `Format::DateTime`, so the calendar arithmetic stays in that library.
    ///
    /// A PROPERTY of the published version and not a clock reading: the same
    /// version answers the same instant on every call, and only publishing a new
    /// version changes what this catalogue reports.
    pub last_update: &'static str,
}

impl ManagedRuleGroupRelease {
    /// [`Self::last_update`] as the modelled `Timestamp`, or `None` when the text
    /// is not an instant that library can read — which leaves the optional
    /// `ManagedRuleGroupVersion$LastUpdateTimestamp` member off the response
    /// rather than reporting an instant nobody published.
    pub fn last_update_timestamp(&self) -> Option<::aws_smithy_types::DateTime> {
        ::aws_smithy_types::DateTime::from_str(
            self.last_update,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
    }
}

/// The managed rule groups this emulator publishes, with the capacity of each.
///
/// A managed rule group is identified by `(VendorName, Name)` and "you can
/// retrieve the required names by calling `ListAvailableManagedRuleGroups`"
/// (`ManagedRuleGroupStatement`'s own doc), so the set of groups a save may
/// reference is a CATALOGUE the service publishes rather than anything the request
/// carries. This is the AWS-vendor catalogue as the WAF Developer Guide's "AWS
/// Managed Rules rule groups list" publishes it: each entry's `Name` and its
/// "Capacity units required" (WCU) column, which is the cost that list states for
/// running the group in a web ACL.
///
/// Kept in the guide's own order (baseline groups, then use-case-specific ones,
/// then the IP-reputation and intelligent-threat-mitigation groups), so a new
/// group the vendor publishes is added where the guide lists it.
///
/// One capacity per group: where the guide states a group's cost per
/// CONFIGURATION (Bot Control, whose targeted inspection level costs more than its
/// common one), the entry carries the group's base figure, because nothing here
/// prices a `ManagedRuleGroupConfigs` setting.
///
/// Each entry also carries the VERSIONS its vendor offers of that group (see
/// [`ManagedRuleGroupDescriptor::versions`]), oldest first: every published group
/// offers at least its initial version, which is what
/// `ListAvailableManagedRuleGroupVersions` lists and what
/// [`ManagedRuleGroupDescriptor::current_default_version`] answers the newest of.
///
/// And it carries what the group is PUBLISHED as — its display title and its
/// one-sentence description, from the same list as its name and capacity, plus the
/// two flags that list implies (see [`descriptor`] and [`versioned`]) — because a
/// managed product is one of these groups as a describe reports it
/// ([`ManagedRuleGroupDescriptor::product`]), not a separate catalogue.
pub const MANAGED_RULE_GROUP_CATALOGUE: &[ManagedRuleGroupDescriptor] = &[
    descriptor(
        "AWSManagedRulesCommonRuleSet",
        700,
        "Core rule set (CRS)",
        "Provides protection against exploitation of a wide range of \
         vulnerabilities, including high risk and commonly occurring \
         vulnerabilities described in OWASP publications.",
        COMMON_RULE_SET_RULES,
    ),
    descriptor(
        "AWSManagedRulesAdminProtectionRuleSet",
        100,
        "Admin protection",
        "Blocks external access to exposed administrative pages.",
        ADMIN_PROTECTION_RULES,
    ),
    descriptor(
        "AWSManagedRulesKnownBadInputsRuleSet",
        200,
        "Known bad inputs",
        "Blocks request patterns that are known to be invalid and are associated \
         with exploitation or discovery of vulnerabilities.",
        KNOWN_BAD_INPUTS_RULES,
    ),
    descriptor(
        "AWSManagedRulesSQLiRuleSet",
        200,
        "SQL database",
        "Blocks request patterns associated with the exploitation of SQL \
         databases, like SQL injection attacks.",
        SQL_DATABASE_RULES,
    ),
    descriptor(
        "AWSManagedRulesLinuxRuleSet",
        200,
        "Linux operating system",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to Linux, including Linux-specific local file \
         inclusion attacks.",
        LINUX_RULES,
    ),
    descriptor(
        "AWSManagedRulesUnixRuleSet",
        100,
        "POSIX operating system",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to POSIX and POSIX-like operating systems.",
        UNIX_RULES,
    ),
    descriptor(
        "AWSManagedRulesWindowsRuleSet",
        200,
        "Windows operating system",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to Windows, like remote execution of PowerShell \
         commands.",
        WINDOWS_RULES,
    ),
    descriptor(
        "AWSManagedRulesPHPRuleSet",
        100,
        "PHP application",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to the use of the PHP programming language, \
         including the injection of unsafe PHP functions.",
        PHP_RULES,
    ),
    descriptor(
        "AWSManagedRulesWordPressRuleSet",
        100,
        "WordPress application",
        "Blocks request patterns associated with the exploitation of \
         vulnerabilities specific to WordPress sites.",
        WORDPRESS_RULES,
    ),
    descriptor(
        "AWSManagedRulesAmazonIpReputationList",
        25,
        "Amazon IP reputation list",
        "Blocks requests from sources that Amazon threat intelligence associates \
         with bots or other threats.",
        IP_REPUTATION_RULES,
    ),
    descriptor(
        "AWSManagedRulesAnonymousIpList",
        50,
        "Anonymous IP list",
        "Blocks requests from services that permit the obfuscation of viewer \
         identity, such as VPNs, proxies, Tor nodes and hosting providers.",
        ANONYMOUS_IP_RULES,
    ),
    versioned(
        "AWSManagedRulesBotControlRuleSet",
        50,
        BOT_CONTROL_VERSIONS,
        NO_RETIRED_VERSIONS,
        "Bot Control",
        "Provides visibility into and control over bot traffic, including common \
         desirable bots and the targeted detection of self-identifying and \
         evasive bots.",
        BOT_CONTROL_RULES,
    ),
    versioned(
        "AWSManagedRulesATPRuleSet",
        50,
        ATP_VERSIONS,
        ATP_RETIRED_VERSIONS,
        "Account takeover prevention",
        "Detects and manages requests that are part of malicious account takeover \
         attempts against your login pages.",
        ATP_RULES,
    ),
    versioned(
        "AWSManagedRulesACFPRuleSet",
        50,
        ACFP_VERSIONS,
        NO_RETIRED_VERSIONS,
        "Account creation fraud prevention",
        "Detects and manages requests that are part of malicious account creation \
         attempts against your account registration pages.",
        ACFP_RULES,
    ),
    versioned(
        "AWSManagedRulesAntiDDoSRuleSet",
        50,
        ANTI_DDOS_VERSIONS,
        NO_RETIRED_VERSIONS,
        "Anti-DDoS",
        "Detects and manages requests that are part of a distributed \
         denial-of-service event against your protected resources.",
        ANTI_DDOS_RULES,
    ),
];

/// The version a group the WAF Developer Guide publishes no version HISTORY for
/// offers: its current one, and no other.
///
/// A managed rule group is offered WITH versions — a recorded
/// `ListAvailableManagedRuleGroupVersions` response for the core rule set carries
/// at least one, and `ManagedRuleGroupStatement$Version` lets a web ACL pin one of
/// any group — so every catalogue entry offers at least this, and a group the guide
/// lists several concurrent versions for carries them instead (see
/// [`BOT_CONTROL_VERSIONS`]). The guide publishes no version NAMES or dates for the
/// groups that carry this one, so both are what THIS catalogue publishes for them
/// rather than anything observed from the real service: one version on offer,
/// stable per group and per call, which is what keeps every published group
/// pinnable and listable, with the one it superseded retired
/// ([`RETIRED_INITIAL_VERSION`]).
const CURRENT_VERSION_ONLY: &[ManagedRuleGroupRelease] =
    &[release("Version_1.1", "2021-03-23T00:00:00Z")];

/// The versions such a group's vendor has RETIRED: the initial version
/// [`CURRENT_VERSION_ONLY`] superseded, which the vendor no longer offers.
///
/// A vendor keeps publishing new versions of a group and expires the ones they
/// supersede, which is why `ManagedRuleGroupStatement$Version` is pinned "until you
/// change it" and why the model gives the save path and the describe their own
/// `WAFExpiredManagedRuleGroupVersionException`. A group that keeps one version on
/// offer therefore has its previous one here: known, so a request that pins it is
/// told it has expired, and unlisted, so a caller rolls forward to what
/// `ListAvailableManagedRuleGroupVersions` does offer. Like the names and dates in
/// [`CURRENT_VERSION_ONLY`], this history is what this catalogue publishes.
const RETIRED_INITIAL_VERSION: &[ManagedRuleGroupRelease] =
    &[release("Version_1.0", "2019-11-25T00:00:00Z")];

/// The retired versions of a group whose vendor has expired none of what it
/// published: every version it ever named is still on offer.
const NO_RETIRED_VERSIONS: &[ManagedRuleGroupRelease] = &[];

/// The versions offered of `AWSManagedRulesBotControlRuleSet`, oldest first.
///
/// The intelligent-threat-mitigation groups below are the ones the WAF Developer
/// Guide publishes a version history for, and the names are the guide's
/// (`Version_<major>.<minor>`). The instant on each release is the one THIS
/// catalogue publishes as that version's last update — stable per version, and
/// ordered with the list — because a vendor's own update instants are not
/// something this emulator can know.
const BOT_CONTROL_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2021-02-09T00:00:00Z"),
    release("Version_2.0", "2022-09-21T00:00:00Z"),
    release("Version_3.0", "2023-10-24T00:00:00Z"),
];

/// The versions offered of `AWSManagedRulesATPRuleSet`: the one its vendor has not
/// retired. See [`BOT_CONTROL_VERSIONS`] and [`ATP_RETIRED_VERSIONS`].
const ATP_VERSIONS: &[ManagedRuleGroupRelease] =
    &[release("Version_3.0", "2023-11-14T00:00:00Z")];

/// The versions of `AWSManagedRulesATPRuleSet` its vendor has RETIRED, oldest
/// first: the two that [`ATP_VERSIONS`] superseded. See
/// [`RETIRED_INITIAL_VERSION`] for why a superseded version is published rather
/// than dropped.
const ATP_RETIRED_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2022-06-14T00:00:00Z"),
    release("Version_2.0", "2023-05-03T00:00:00Z"),
];

/// The versions offered of `AWSManagedRulesACFPRuleSet`. See
/// [`BOT_CONTROL_VERSIONS`].
const ACFP_VERSIONS: &[ManagedRuleGroupRelease] = &[
    release("Version_1.0", "2023-09-26T00:00:00Z"),
    release("Version_2.0", "2024-04-09T00:00:00Z"),
];

/// The versions offered of `AWSManagedRulesAntiDDoSRuleSet`. See
/// [`BOT_CONTROL_VERSIONS`].
const ANTI_DDOS_VERSIONS: &[ManagedRuleGroupRelease] =
    &[release("Version_1.0", "2025-05-14T00:00:00Z")];

/// One [`MANAGED_RULE_GROUP_CATALOGUE`] entry, of the AWS vendor, offered with
/// [`CURRENT_VERSION_ONLY`] and carrying [`RETIRED_INITIAL_VERSION`]: a group the
/// guide publishes no version history for, so its vendor keeps one version on offer
/// at a time — it supports no choice of named versions and is not one of the
/// intelligent-threat-mitigation groups either (see [`versioned`]).
const fn descriptor(
    name: &'static str,
    capacity: i64,
    product_title: &'static str,
    product_description: &'static str,
    rules: &'static [PublishedManagedRule],
) -> ManagedRuleGroupDescriptor {
    ManagedRuleGroupDescriptor {
        vendor_name: AWS_MANAGED_RULE_GROUP_VENDOR,
        name,
        capacity,
        product_title,
        product_description,
        is_versioning_supported: false,
        is_advanced_managed_rule_set: false,
        versions: CURRENT_VERSION_ONLY,
        expired_versions: RETIRED_INITIAL_VERSION,
        rules,
    }
}

/// [`descriptor`] for a group whose vendor offers `versions` of it, oldest first,
/// and has retired `expired_versions` of it, also oldest first.
///
/// Those are the intelligent-threat-mitigation groups: the guide publishes a
/// version history for exactly them, and they are the ones it describes as the
/// advanced protections — so an entry built here both supports versioning and is
/// an advanced managed rule set.
const fn versioned(
    name: &'static str,
    capacity: i64,
    versions: &'static [ManagedRuleGroupRelease],
    expired_versions: &'static [ManagedRuleGroupRelease],
    product_title: &'static str,
    product_description: &'static str,
    rules: &'static [PublishedManagedRule],
) -> ManagedRuleGroupDescriptor {
    ManagedRuleGroupDescriptor {
        vendor_name: AWS_MANAGED_RULE_GROUP_VENDOR,
        name,
        capacity,
        product_title,
        product_description,
        is_versioning_supported: true,
        is_advanced_managed_rule_set: true,
        versions,
        expired_versions,
        rules,
    }
}

/// One rule of a published group, named `name`, that BLOCKS what it matches.
const fn block(name: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Block,
        published_label: None,
    }
}

/// One rule of a published group, named `name`, that COUNTS what it matches. See
/// [`block`].
const fn count(name: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Count,
        published_label: None,
    }
}

/// One rule of a published group, named `name`, that CHALLENGES what it matches.
/// See [`block`].
const fn challenge(name: &'static str) -> PublishedManagedRule {
    PublishedManagedRule {
        name,
        action: PublishedRuleAction::Challenge,
        published_label: None,
    }
}

// ---------------------------------------------------------------------------
// The rules each published group runs
// ---------------------------------------------------------------------------
//
// One list per catalogue entry, in the order the WAF Developer Guide's rule list
// for that group gives them: each rule's name and the action that list states for
// it (`RuleSummary` is "high-level information about a Rule, returned by
// operations like DescribeManagedRuleGroup" — a name and an action, and nothing
// about how the rule matches, which no WAFv2 response reports for a vendor's
// group). They are the VENDOR's rules, so they live here beside the group's
// capacity and versions rather than in the state struct, and nothing a caller does
// changes one.

/// The rules of `AWSManagedRulesCommonRuleSet`.
const COMMON_RULE_SET_RULES: &[PublishedManagedRule] = &[
    block("NoUserAgent_HEADER"),
    block("UserAgent_BadBots_HEADER"),
    block("SizeRestrictions_QUERYSTRING"),
    block("SizeRestrictions_Cookie_HEADER"),
    block("SizeRestrictions_BODY"),
    block("SizeRestrictions_URIPATH"),
    block("EC2MetaDataSSRF_BODY"),
    block("EC2MetaDataSSRF_COOKIE"),
    block("EC2MetaDataSSRF_URIPATH"),
    block("EC2MetaDataSSRF_QUERYARGUMENTS"),
    block("GenericLFI_QUERYARGUMENTS"),
    block("GenericLFI_URIPATH"),
    block("GenericLFI_BODY"),
    block("RestrictedExtensions_URIPATH"),
    block("RestrictedExtensions_QUERYARGUMENTS"),
    block("GenericRFI_QUERYARGUMENTS"),
    block("GenericRFI_BODY"),
    block("GenericRFI_URIPATH"),
    block("CrossSiteScripting_COOKIE"),
    block("CrossSiteScripting_QUERYARGUMENTS"),
    block("CrossSiteScripting_BODY"),
    block("CrossSiteScripting_URIPATH"),
];

/// The rules of `AWSManagedRulesAdminProtectionRuleSet`.
const ADMIN_PROTECTION_RULES: &[PublishedManagedRule] = &[block("AdminProtection_URIPATH")];

/// The rules of `AWSManagedRulesKnownBadInputsRuleSet`.
const KNOWN_BAD_INPUTS_RULES: &[PublishedManagedRule] = &[
    block("JavaDeserializationRCE_HEADER"),
    block("JavaDeserializationRCE_BODY"),
    block("JavaDeserializationRCE_URIPATH"),
    block("JavaDeserializationRCE_QUERYSTRING"),
    block("Host_localhost_HEADER"),
    block("PROPFIND_METHOD"),
    block("ExploitablePaths_URIPATH"),
    block("Log4JRCE_HEADER"),
    block("Log4JRCE_QUERYSTRING"),
    block("Log4JRCE_BODY"),
    block("Log4JRCE_URIPATH"),
];

/// The rules of `AWSManagedRulesSQLiRuleSet`.
const SQL_DATABASE_RULES: &[PublishedManagedRule] = &[
    block("SQLi_QUERYARGUMENTS"),
    block("SQLiExtendedPatterns_QUERYARGUMENTS"),
    block("SQLi_BODY"),
    block("SQLiExtendedPatterns_BODY"),
    block("SQLi_COOKIE"),
    block("SQLi_URIPATH"),
];

/// The rules of `AWSManagedRulesLinuxRuleSet`.
const LINUX_RULES: &[PublishedManagedRule] = &[
    block("LFI_URIPATH"),
    block("LFI_QUERYSTRING"),
    block("LFI_HEADER"),
];

/// The rules of `AWSManagedRulesUnixRuleSet`.
const UNIX_RULES: &[PublishedManagedRule] = &[
    block("UNIXShellCommandsVariables_QUERYARGUMENTS"),
    block("UNIXShellCommandsVariables_BODY"),
];

/// The rules of `AWSManagedRulesWindowsRuleSet`.
const WINDOWS_RULES: &[PublishedManagedRule] = &[
    block("WindowsShellCommandsVariables_QUERYARGUMENTS"),
    block("WindowsShellCommandsVariables_BODY"),
    block("PowerShellCommandsVariables_QUERYARGUMENTS"),
    block("PowerShellCommandsVariables_BODY"),
];

/// The rules of `AWSManagedRulesPHPRuleSet`.
const PHP_RULES: &[PublishedManagedRule] = &[
    block("PHPHighRiskMethodsVariables_HEADER"),
    block("PHPHighRiskMethodsVariables_QUERYSTRING"),
    block("PHPHighRiskMethodsVariables_BODY"),
];

/// The rules of `AWSManagedRulesWordPressRuleSet`.
const WORDPRESS_RULES: &[PublishedManagedRule] = &[
    block("WordPressExploitableCommands_QUERYSTRING"),
    block("WordPressExploitablePaths_URIPATH"),
];

/// The rules of `AWSManagedRulesAmazonIpReputationList`.
const IP_REPUTATION_RULES: &[PublishedManagedRule] = &[
    block("AWSManagedIPReputationList"),
    block("AWSManagedReconnaissanceList"),
    block("AWSManagedIPDDoSList"),
];

/// The rules of `AWSManagedRulesAnonymousIpList`.
const ANONYMOUS_IP_RULES: &[PublishedManagedRule] =
    &[block("AnonymousIPList"), block("HostingProviderIPList")];

/// The rules of `AWSManagedRulesBotControlRuleSet`, whose bot-category rules count
/// the traffic they recognise and whose bot-signal rules block it.
const BOT_CONTROL_RULES: &[PublishedManagedRule] = &[
    count("CategoryAdvertising"),
    count("CategoryArchiver"),
    count("CategoryContentFetcher"),
    count("CategoryEmailClient"),
    count("CategoryHttpLibrary"),
    count("CategoryLinkChecker"),
    count("CategoryMiscellaneous"),
    count("CategoryMonitoring"),
    block("CategoryScrapingFramework"),
    count("CategorySearchEngine"),
    count("CategorySecurity"),
    count("CategorySeo"),
    count("CategorySocialMedia"),
    block("SignalAutomatedBrowser"),
    count("SignalKnownBotDataCenter"),
    block("SignalNonBrowserUserAgent"),
];

/// The rules of `AWSManagedRulesATPRuleSet`.
const ATP_RULES: &[PublishedManagedRule] = &[
    count("AllRequests"),
    block("UnsupportedCognitoIDP"),
    block("AttributeCompromisedCredentials"),
    block("AttributeUsernameTraversal"),
    block("AttributePasswordTraversal"),
    block("VolumetricIpHigh"),
    block("VolumetricSession"),
    block("SignalMissingCredential"),
    block("TokenRejected"),
];

/// The rules of `AWSManagedRulesACFPRuleSet`.
const ACFP_RULES: &[PublishedManagedRule] = &[
    count("AllRequests"),
    block("UnsupportedCognitoIDP"),
    block("AttributeCompromisedCredentials"),
    block("AttributeUsernameTraversal"),
    block("AttributePasswordTraversal"),
    block("VolumetricIpHigh"),
    block("VolumetricSession"),
    block("VolumetricPhoneNumberHigh"),
    block("SignalMissingCredential"),
    block("TokenRejected"),
];

/// The rules of `AWSManagedRulesAntiDDoSRuleSet`, which challenge the traffic they
/// recognise rather than blocking it outright.
const ANTI_DDOS_RULES: &[PublishedManagedRule] = &[
    challenge("ChallengeDDoSRequests"),
    challenge("ChallengeAllDuringEvent"),
];

/// One version of a group, named `name` and last updated at `last_update` (see
/// [`ManagedRuleGroupRelease::last_update`]).
const fn release(name: &'static str, last_update: &'static str) -> ManagedRuleGroupRelease {
    ManagedRuleGroupRelease { name, last_update }
}

/// The published managed rule group a `(VendorName, Name)` pair names, if the
/// catalogue holds one.
///
/// Both members are matched exactly: `ManagedRuleGroupStatement$Name` targets
/// `EntityName`, whose names WAF treats as case-sensitive, and the vendor name is
/// the vendor's own.
pub fn managed_rule_group(
    vendor_name: &str,
    name: &str,
) -> Option<&'static ManagedRuleGroupDescriptor> {
    MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .find(|group| group.vendor_name == vendor_name && group.name == name)
}

/// The refusal a rule list earns by PINNING a managed rule group version its
/// vendor has retired, if one of its rules pins one.
///
/// `ManagedRuleGroupStatement$Version` is "the version of the managed rule group to
/// use. If you specify this, the version setting is fixed until you change it", and
/// WAF resolves it when the rule is SAVED — the same moment every reference
/// [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`] resolves at
/// — so a rule pinning a version the publishing vendor no longer offers
/// ([`ManagedVersionState::Expired`]) is refused with the modelled
/// `WAFExpiredManagedRuleGroupVersionException` and nothing is stored, rather than
/// kept as though the pin were live. A version the group never published is NOT
/// this refusal, and neither is a statement naming no version at all: that one runs
/// "the vendor's default version" (the member's own doc), which is by definition
/// offered.
///
/// Walked with an explicit worklist, like [`misplaced_rule_group_statement`]: the
/// tree is supplied by the request.
pub fn expired_managed_rule_group_version(
    rules: &[Rule],
) -> Option<WAFExpiredManagedRuleGroupVersionException> {
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some((managed, version)) = statement
            .managed_rule_group_statement
            .as_ref()
            .and_then(|managed| {
                managed
                    .version
                    .as_deref()
                    .map(|version| (&**managed, version))
            })
            .filter(|(managed, version)| {
                managed_rule_group(&managed.vendor_name, &managed.name)
                    .is_some_and(|group| group.version_state(version) == ManagedVersionState::Expired)
            })
        {
            return Some(expired_managed_rule_group_version_exception(
                &managed.vendor_name,
                &managed.name,
                version,
            ));
        }
        pending.extend(nested_statements(statement));
    }
    None
}

/// The statements nested directly inside `statement`, in no particular order.
///
/// One place names every nesting the model has, so the depth validator and both
/// tree walks below agree on what "nested" means.
///
/// [`nested_statements_mut`] is its mutable twin, arm for arm: a nesting the model
/// grows has to be added to BOTH, or a default would be applied on one walk and
/// not the other.
fn nested_statements(statement: &Statement) -> Vec<&Statement> {
    let mut nested: Vec<&Statement> = Vec::new();
    if let Some(and) = statement.and_statement.as_ref() {
        nested.extend(and.statements.iter());
    }
    if let Some(or) = statement.or_statement.as_ref() {
        nested.extend(or.statements.iter());
    }
    if let Some(not) = statement.not_statement.as_ref() {
        nested.push(&not.statement);
    }
    if let Some(scope_down) = statement
        .rate_based_statement
        .as_ref()
        .and_then(|rate| rate.scope_down_statement.as_deref())
    {
        nested.push(scope_down);
    }
    if let Some(scope_down) = statement
        .managed_rule_group_statement
        .as_ref()
        .and_then(|managed| managed.scope_down_statement.as_deref())
    {
        nested.push(scope_down);
    }
    nested
}

/// [`nested_statements`], mutably: the statements nested directly inside
/// `statement`, in no particular order.
///
/// Used by [`apply_rule_defaults`] to reach every statement of a tree in place.
/// Each arm borrows a DIFFERENT field of `statement`, so the borrows are disjoint.
/// Kept arm for arm in step with [`nested_statements`]: a nesting the model grows
/// belongs in both.
fn nested_statements_mut(statement: &mut Statement) -> Vec<&mut Statement> {
    let mut nested: Vec<&mut Statement> = Vec::new();
    if let Some(and) = statement.and_statement.as_mut() {
        nested.extend(and.statements.iter_mut());
    }
    if let Some(or) = statement.or_statement.as_mut() {
        nested.extend(or.statements.iter_mut());
    }
    if let Some(not) = statement.not_statement.as_mut() {
        nested.push(&mut not.statement);
    }
    if let Some(scope_down) = statement
        .rate_based_statement
        .as_mut()
        .and_then(|rate| rate.scope_down_statement.as_deref_mut())
    {
        nested.push(scope_down);
    }
    if let Some(scope_down) = statement
        .managed_rule_group_statement
        .as_mut()
        .and_then(|managed| managed.scope_down_statement.as_deref_mut())
    {
        nested.push(scope_down);
    }
    nested
}

/// Refuse a rule list whose statements nest deeper than
/// [`MAX_STATEMENT_NESTING_DEPTH`]. `Err` carries the reason, for the refusal's
/// message.
///
/// Walked with an explicit worklist rather than by recursion: the tree is supplied
/// by the request, so the check that bounds its depth must not itself consume
/// stack in proportion to that depth.
pub fn validate_statement_nesting(rules: &[Rule]) -> Result<(), String> {
    let mut pending: Vec<(&Statement, u32)> = rules
        .iter()
        .map(|rule| (&rule.statement, 1_u32))
        .collect();
    while let Some((statement, depth)) = pending.pop() {
        if depth > MAX_STATEMENT_NESTING_DEPTH {
            return Err(format!(
                "a rule statement nests {depth} levels deep; WAF allows at most \
                 {MAX_STATEMENT_NESTING_DEPTH}"
            ));
        }
        pending.extend(
            nested_statements(statement)
                .into_iter()
                .map(|nested| (nested, depth + 1)),
        );
    }
    Ok(())
}

/// One kind of statement that runs a whole RULE GROUP: how to spot it, what to
/// call it in a refusal's reason, and the `ParameterExceptionField` value that
/// names it.
struct RuleGroupRunningStatement {
    /// The statement's shape name, for the refusal's reason.
    shape_name: &'static str,
    /// The `ParameterExceptionField` value naming this statement.
    field: &'static str,
    /// `true` when `statement` carries this kind of statement.
    present: fn(&Statement) -> bool,
}

/// Which statements run a whole rule group: the customer-owned
/// `RuleGroupReferenceStatement` and the vendor-published
/// `ManagedRuleGroupStatement`.
///
/// One list, because the model states the SAME placement rule on both and the
/// check below is defined over that rule rather than over either statement.
const RULE_GROUP_RUNNING_STATEMENTS: &[RuleGroupRunningStatement] = &[
    RuleGroupRunningStatement {
        shape_name: "RuleGroupReferenceStatement",
        field: FIELD_RULE_GROUP_REFERENCE_STATEMENT,
        present: |statement| statement.rule_group_reference_statement.is_some(),
    },
    RuleGroupRunningStatement {
        shape_name: "ManagedRuleGroupStatement",
        field: FIELD_MANAGED_RULE_SET_STATEMENT,
        present: |statement| statement.managed_rule_group_statement.is_some(),
    },
];

/// Where a statement that runs a whole rule group may appear, if `rules` puts one
/// somewhere it may not. `Some` carries the `ParameterExceptionField` value naming
/// the statement at fault and the reason, for the refusal.
///
/// The model states the placement rule on each such statement itself, in the same
/// words: "You cannot nest a `RuleGroupReferenceStatement`, for example for use
/// inside a `NotStatement` or `OrStatement`. You cannot use a rule group reference
/// statement inside another rule group. You can only reference a rule group as a
/// top-level statement within a rule that you define in a web ACL"
/// (`smithy_query(query="doc",shape="RuleGroupReferenceStatement")`) and "You
/// cannot nest a `ManagedRuleGroupStatement`, for example for use inside a
/// `NotStatement` or `OrStatement`. You cannot use a managed rule group inside
/// another rule group. You can only reference a managed rule group as a top-level
/// statement within a rule that you define in a web ACL"
/// (`smithy_query(query="doc",shape="ManagedRuleGroupStatement")`). So a statement
/// that runs a rule group — whoever owns that group — is well placed ONLY as a
/// rule's own top-level statement, and only in a web ACL: `top_level_allowed` is
/// `true` for a web ACL save and `false` for a rule group save. A misplacement is a
/// parameter WAF does not accept rather than a dangling reference, so it is
/// answered with `WAFInvalidParameterException` ("Your nested statement isn't
/// valid. You might have tried to nest a statement that can’t be nested", from that
/// error's own doc) — and it is judged BEFORE the reference is resolved, because a
/// well-placed reference is what resolution is defined over.
///
/// A managed rule group's own `ScopeDownStatement` is NOT a misplacement: it is the
/// nested statement that managed statement is modelled to carry ("an optional
/// nested statement that narrows the scope of the web requests that are evaluated
/// by the managed rule group"), and what this refuses is a group-running statement
/// reached THROUGH a nesting.
///
/// Walked with an explicit worklist, like [`validate_statement_nesting`]: the tree
/// is supplied by the request.
pub fn misplaced_rule_group_statement(
    rules: &[Rule],
    top_level_allowed: bool,
) -> Option<(&'static str, String)> {
    for rule in rules {
        for kind in RULE_GROUP_RUNNING_STATEMENTS {
            if !top_level_allowed && (kind.present)(&rule.statement) {
                return Some((
                    kind.field,
                    format!(
                        "the rule {:?} carries a {}; a rule group can only be \
                         referenced from a rule of a web ACL, not from a rule of \
                         another rule group",
                        rule.name, kind.shape_name
                    ),
                ));
            }
        }
        let mut pending: Vec<&Statement> = nested_statements(&rule.statement);
        while let Some(statement) = pending.pop() {
            for kind in RULE_GROUP_RUNNING_STATEMENTS {
                if (kind.present)(statement) {
                    return Some((
                        kind.field,
                        format!(
                            "the rule {:?} nests a {} inside another statement; a \
                             rule group can only be referenced by a rule's \
                             top-level statement",
                            rule.name, kind.shape_name
                        ),
                    ));
                }
            }
            pending.extend(nested_statements(statement));
        }
    }
    None
}

/// `true` when `statement`, or anything nested inside it, is a reference statement
/// naming `arn`.
///
/// The three reference-bearing statements of this model that name a resource of
/// this account — `IPSetReferenceStatement`, `RuleGroupReferenceStatement` and
/// `RegexPatternSetReferenceStatement` — are all checked, because an ARN names one
/// resource of one kind: whichever kind `arn` belongs to, the statement that could
/// carry it is the one for that kind, and no other statement can name it.
///
/// `depth_budget` is the number of levels left to descend, so this cannot recurse
/// further than [`MAX_STATEMENT_NESTING_DEPTH`] even if a tree somehow reached the
/// store without passing [`validate_statement_nesting`].
fn statement_references_arn(statement: &Statement, arn: &str, depth_budget: u32) -> bool {
    if depth_budget == 0 {
        return false;
    }
    let names_arn = statement
        .ip_set_reference_statement
        .as_ref()
        .is_some_and(|reference| reference.arn == arn)
        || statement
            .rule_group_reference_statement
            .as_ref()
            .is_some_and(|reference| reference.arn == arn)
        || statement
            .regex_pattern_set_reference_statement
            .as_ref()
            .is_some_and(|reference| reference.arn == arn);
    if names_arn {
        return true;
    }
    nested_statements(statement)
        .into_iter()
        .any(|nested| statement_references_arn(nested, arn, depth_budget - 1))
}

// ---------------------------------------------------------------------------
// RuleGroup permission policies
//
// `PutPermissionPolicy` attaches an IAM policy to the rule group its `ResourceArn`
// names and `GetPermissionPolicy` reports it back; the attached document itself
// lives on [`RuleGroupEntry::permission_policy`], so one copy serves every reader.
// The restrictions checked below are the ones `PutPermissionPolicy` and
// `WAFInvalidPermissionPolicyException` document, nothing wider: the document is
// only CHECKED here, never rewritten.
// ---------------------------------------------------------------------------

/// The IAM policy language version a permission policy must declare.
///
/// Source: `PutPermissionPolicy`'s own documentation — "The policy must be composed
/// using IAM Policy version 2012-10-17."
pub const PERMISSION_POLICY_VERSION: &str = "2012-10-17";

/// The one `Effect` a permission policy statement may carry.
///
/// Source: `PutPermissionPolicy`'s own documentation — "`Effect` must specify
/// `Allow`."
pub const PERMISSION_POLICY_EFFECT: &str = "Allow";

/// Every `Action` a permission policy has to grant.
///
/// Source: `PutPermissionPolicy`'s own documentation — "`Action` must specify
/// `wafv2:CreateWebACL`, `wafv2:UpdateWebACL`, and
/// `wafv2:PutFirewallManagerRuleGroups`."
pub const PERMISSION_POLICY_REQUIRED_ACTIONS: &[&str] = &[
    "wafv2:CreateWebACL",
    "wafv2:UpdateWebACL",
    "wafv2:PutFirewallManagerRuleGroups",
];

/// The `Action`s a permission policy may grant BESIDES
/// [`PERMISSION_POLICY_REQUIRED_ACTIONS`], and the only ones.
///
/// Source: `PutPermissionPolicy`'s own documentation — "… and may optionally specify
/// `wafv2:GetRuleGroup`. WAF rejects any extra actions or wildcard actions in the
/// policy."
pub const PERMISSION_POLICY_OPTIONAL_ACTIONS: &[&str] = &["wafv2:GetRuleGroup"];

/// Whether `action` is one a permission policy may grant.
///
/// The two lists hold the literal actions the documentation names, so an action
/// outside them is refused — including any form carrying a `*`, which is never one
/// of the literals and so is rejected as the documentation requires, however wide or
/// narrow the wildcard is.
fn permission_policy_action_is_allowed(action: &str) -> bool {
    PERMISSION_POLICY_REQUIRED_ACTIONS.contains(&action)
        || PERMISSION_POLICY_OPTIONAL_ACTIONS.contains(&action)
}

/// Whether `policy` is a permission policy WAF accepts: a JSON object declaring IAM
/// policy version [`PERMISSION_POLICY_VERSION`] whose `Statement` — one statement
/// object, or a non-empty list of them — every entry of satisfies
/// [`permission_policy_statement_actions`], and which between them grant every
/// [`PERMISSION_POLICY_REQUIRED_ACTIONS`] entry.
///
/// Source: the restrictions `PutPermissionPolicy` and
/// `WAFInvalidPermissionPolicyException` document. What gets stored is the caller's
/// own text, byte for byte.
pub fn permission_policy_is_valid(policy: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(document) = document.as_object() else {
        return false;
    };
    if document
        .get("Version")
        .and_then(::serde_json::Value::as_str)
        != Some(PERMISSION_POLICY_VERSION)
    {
        return false;
    }
    let statements: Vec<&::serde_json::Value> = match document.get("Statement") {
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        Some(::serde_json::Value::Array(list)) => list.iter().collect(),
        // A policy with no `Statement`, or one that is neither a statement nor a
        // list of statements, carries no `Effect`, `Action` or `Principal` at all.
        _ => return false,
    };
    if statements.is_empty() {
        return false;
    }
    let mut granted: Vec<&str> = Vec::new();
    for statement in statements {
        match permission_policy_statement_actions(statement) {
            Some(actions) => granted.extend(actions),
            None => return false,
        }
    }
    // The required actions are required of the POLICY: a document may spread them
    // over several statements, so they are counted once every statement has been
    // found to be within the restrictions.
    PERMISSION_POLICY_REQUIRED_ACTIONS
        .iter()
        .all(|required| granted.contains(required))
}

/// The actions one `Statement` of a permission policy grants, or `None` when the
/// statement is outside the documented restrictions: it must carry an `Effect` of
/// [`PERMISSION_POLICY_EFFECT`], a non-empty `Principal`, an `Action` — one action or
/// a non-empty list of them — naming only actions
/// [`permission_policy_action_is_allowed`] admits, and no `Resource`.
///
/// `NotAction`, `NotPrincipal` and `NotResource` are refused with the member they
/// negate: a statement carrying one names no `Action` or `Principal`, or names a
/// resource the policy "must not include".
fn permission_policy_statement_actions(statement: &::serde_json::Value) -> Option<Vec<&str>> {
    let statement = statement.as_object()?;
    if statement.contains_key("Resource") || statement.contains_key("NotResource") {
        return None;
    }
    if statement.get("Effect").and_then(::serde_json::Value::as_str)
        != Some(PERMISSION_POLICY_EFFECT)
    {
        return None;
    }
    if statement.contains_key("NotPrincipal") || statement.contains_key("NotAction") {
        return None;
    }
    if !permission_policy_principal_is_present(statement.get("Principal")) {
        return None;
    }
    let actions: Vec<&::serde_json::Value> = match statement.get("Action")? {
        action @ ::serde_json::Value::String(_) => vec![action],
        ::serde_json::Value::Array(list) => list.iter().collect(),
        _ => return None,
    };
    if actions.is_empty() {
        return None;
    }
    actions
        .into_iter()
        .map(|action| {
            let action = action.as_str()?;
            permission_policy_action_is_allowed(action).then_some(action)
        })
        .collect()
}

/// Whether a statement names a `Principal`: a non-blank string, or a map of
/// principal kinds (`AWS`, `Service`, …) whose every entry carries a non-blank value
/// or a non-empty list of them.
fn permission_policy_principal_is_present(principal: Option<&::serde_json::Value>) -> bool {
    match principal {
        Some(::serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(::serde_json::Value::Object(kinds)) => {
            !kinds.is_empty() && kinds.values().all(permission_policy_principal_value)
        }
        _ => false,
    }
}

/// Whether one entry of a `Principal` map names a principal.
fn permission_policy_principal_value(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(value) => !value.trim().is_empty(),
        ::serde_json::Value::Array(values) => {
            !values.is_empty() && values.iter().all(permission_policy_principal_value)
        }
        _ => false,
    }
}

/// A modelled timestamp as milliseconds since the Unix epoch, so a `TimeWindow`
/// endpoint and a stored observation ([`BotRequestRecord::observed_at_millis`])
/// compare as plain numbers. Sub-millisecond precision is truncated, which is the
/// resolution a traffic window is reported at.
pub fn date_time_millis(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// A fresh lowercase, hyphenated UUIDv4 — the form every `EntityId` and
/// `LockToken` in this model takes
/// (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`, from
/// `smithy_query(query="shape",shape="LockToken")`).
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2`
/// crate) and hex-encoded (the `hex` crate), the way this repo's other services
/// mint a UUID-shaped id (see
/// `services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs`): the
/// OS-seeded randomness a `RandomState` draws, the system clock in nanoseconds,
/// and a process-wide counter, so two calls in the same nanosecond cannot collide.
/// The version nibble and the variant bits RFC 4122 §4.4 fixes are then set, so
/// the value is a well-formed version-4 UUID. An id or a lock token names a
/// resource; a credential would come from `ctx.credential_store()`, never here.
pub fn new_uuid() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::collections::hash_map::RandomState;
    use ::std::hash::BuildHasher;

    static SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are
    // `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..]
    )
}

/// The AWS region the request was signed for, or [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope to read one from.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Record what a state-changing WAFv2 call did, as the audit trail a mutation
/// leaves behind.
///
/// `outcome` is what the service decided — `"created"`, `"updated"`, `"deleted"`,
/// or the modelled error it answered — so a refused mutation is as visible as an
/// applied one. The record carries only "what was called": the operation and the
/// resource it names. Nothing from the caller's credential is recorded, not even
/// the access key id the request presented, and the request body never enters the
/// record.
pub fn audit(operation: &str, scope: &str, name: &str, id: &str, outcome: &str) {
    ::tracing::info!(
        target: "wafv2::audit",
        operation,
        scope,
        name,
        id,
        outcome,
        "wafv2 state-changing request"
    );
}

/// `true` when `value` has the shape of a wafv2 resource ARN:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`.
///
/// `GetWebACL` separates the two ARN failures the model names: an ARN that is
/// malformed is `WAFInvalidParameterException` ("Your request references an ARN
/// that is malformed", from that error's own doc), while a well-formed ARN naming
/// nothing stored is `WAFNonexistentItemException`.
pub fn is_wafv2_resource_arn(value: &str) -> bool {
    let mut segments = value.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some("wafv2"),
        Some(_region),
        Some(_account),
        Some(resource),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return false;
    };
    let resource_parts: Vec<&str> = resource.split('/').collect();
    !partition.is_empty()
        && resource_parts.len() == 4
        && matches!(resource_parts[0], "global" | "regional")
        && resource_parts[1..].iter().all(|part| !part.is_empty())
}

/// One form of protected-resource ARN a web ACL can be put in front of, and the
/// `ResourceType` value that form names.
#[derive(Debug)]
pub struct ProtectedResourceForm {
    /// The ARN's service-namespace segment (`elasticloadbalancing`, `appsync`, …).
    pub service: &'static str,
    /// The ARN's resource part, segment by segment: `Some(literal)` must match
    /// that segment exactly, `None` is any ONE non-empty segment — a name or an
    /// id the owning service minted.
    pub segments: &'static [Option<&'static str>],
    /// The `ResourceType` value this form names.
    pub resource_type: &'static str,
}

impl ProtectedResourceForm {
    /// `true` when `service` and the slash-separated `parts` of an ARN's resource
    /// part are this form.
    fn matches(&self, service: &str, parts: &[&str]) -> bool {
        self.service == service
            && parts.len() == self.segments.len()
            && parts
                .iter()
                .zip(self.segments)
                .all(|(part, segment)| match segment {
                    Some(literal) => part == literal,
                    None => !part.is_empty(),
                })
    }
}

/// Every resource form a web ACL can be put in front of through this API.
///
/// `AssociateWebACLRequest$ResourceArn` gives the ARN format of each one
/// (`smithy_query(query="doc",shape="AssociateWebACLRequest",member="ResourceArn")`:
/// "the ARN must be in one of the following formats"), and `ResourceType`
/// (`smithy_query(query="shape",shape="ResourceType")`) names the same eight kinds,
/// so this table is those two lists joined — the catalogue of what a web ACL can
/// protect, and the only place a protected resource's kind is decided.
///
/// A CloudFront distribution is deliberately absent: it is bound to a web ACL
/// through the distribution's own configuration, not through this API ("use this
/// for all resource types except for Amazon CloudFront distributions", the
/// operation's own doc), and `ResourceType` does not list it either.
pub const PROTECTED_RESOURCE_FORMS: &[ProtectedResourceForm] = &[
    ProtectedResourceForm {
        service: "elasticloadbalancing",
        // `loadbalancer/app/<load-balancer-name>/<load-balancer-id>`: the `app`
        // segment is an APPLICATION Load Balancer, which is the only kind of load
        // balancer the list names.
        segments: &[Some("loadbalancer"), Some("app"), None, None],
        resource_type: "APPLICATION_LOAD_BALANCER",
    },
    ProtectedResourceForm {
        service: "apigateway",
        // `/restapis/<api-id>/stages/<stage-name>`, whose leading slash is dropped
        // before the parts are compared.
        segments: &[Some("restapis"), None, Some("stages"), None],
        resource_type: "API_GATEWAY",
    },
    ProtectedResourceForm {
        service: "appsync",
        // `apis/<GraphQLApiId>`
        segments: &[Some("apis"), None],
        resource_type: "APPSYNC",
    },
    ProtectedResourceForm {
        service: "cognito-idp",
        // `userpool/<user-pool-id>`
        segments: &[Some("userpool"), None],
        resource_type: "COGNITO_USER_POOL",
    },
    ProtectedResourceForm {
        service: "apprunner",
        // `service/<apprunner-service-name>/<apprunner-service-id>`
        segments: &[Some("service"), None, None],
        resource_type: "APP_RUNNER_SERVICE",
    },
    ProtectedResourceForm {
        service: "ec2",
        // `verified-access-instance/<instance-id>`
        segments: &[Some("verified-access-instance"), None],
        resource_type: "VERIFIED_ACCESS_INSTANCE",
    },
    ProtectedResourceForm {
        service: "amplify",
        // `apps/<app-id>`
        segments: &[Some("apps"), None],
        resource_type: "AMPLIFY",
    },
    ProtectedResourceForm {
        service: "bedrock-agentcore",
        // `gateway/<gateway-id>`
        segments: &[Some("gateway"), None],
        resource_type: "AGENTCORE_GATEWAY",
    },
];

/// The `ResourceType` a listing selects when the request names none: "if you don't
/// provide a resource type, the call uses the resource type
/// APPLICATION_LOAD_BALANCER" (`ListResourcesForWebACLRequest$ResourceType`).
pub const DEFAULT_RESOURCE_TYPE: &str = "APPLICATION_LOAD_BALANCER";

/// The `ResourceType` of the resource `arn` names, or `None` when a web ACL cannot
/// be put in front of it.
///
/// `None` covers both ways an ARN fails this parameter, which WAF reports as one
/// refusal ("your request references an ARN that is malformed, or corresponds to a
/// resource with which a web ACL can't be associated",
/// `WAFInvalidParameterException`): a string that is not an ARN at all, and a
/// well-formed ARN of a resource kind [`PROTECTED_RESOURCE_FORMS`] does not
/// catalogue.
pub fn protected_resource_type(arn: &str) -> Option<&'static str> {
    let mut segments = arn.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(_region),
        Some(_account),
        Some(resource),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() {
        return None;
    }
    // An API Gateway stage's resource part begins with a slash
    // (`::/restapis/<api-id>/stages/<stage-name>`); every other form does not.
    let parts: Vec<&str> = resource.trim_start_matches('/').split('/').collect();
    PROTECTED_RESOURCE_FORMS
        .iter()
        .find(|form| form.matches(service, &parts))
        .map(|form| form.resource_type)
}

/// The region segment of a scope's ARNs.
///
/// A CLOUDFRONT-scoped resource is always reached through one endpoint — "API and
/// SDKs - For all calls, use the Region endpoint us-east-1" (the `Scope` member
/// doc) — so its ARN carries that region whatever the caller signed for.
fn arn_region<'a>(scope: &str, request_region: &'a str) -> &'a str {
    if scope == SCOPE_CLOUDFRONT {
        DEFAULT_REGION
    } else {
        request_region
    }
}

/// The first resource segment of a wafv2 ARN: `global` for a CLOUDFRONT-scoped
/// resource, `regional` for a REGIONAL one.
fn scope_path(scope: &str) -> &'static str {
    if scope == SCOPE_CLOUDFRONT {
        "global"
    } else {
        "regional"
    }
}

/// `arn:aws:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`, where
/// `kind` is the resource type segment (`ipset`, `webacl`, …).
pub fn resource_arn(
    scope: &str,
    request_region: &str,
    kind: &str,
    name: &str,
    id: &str,
) -> String {
    ::engine::emulator_arn(
        "wafv2",
        arn_region(scope, request_region),
        &format!("{}/{kind}/{name}/{id}", scope_path(scope)),
    )
}

/// `awswaf:<account ID>:<kind>:<entity name>:` — the prefix the `LabelNamespace`
/// member of a label-bearing entity specifies for every label a rule in it adds.
///
/// One helper for every entity kind, because both docs give the same shape with
/// only the kind segment differing: `WebACL$LabelNamespace` is
/// "awswaf:<account ID>:webacl:<web ACL name>:" and `RuleGroup$LabelNamespace` is
/// "awswaf:<account ID>:rulegroup:<rule group name>:". `kind` is the same resource
/// segment an ARN carries, so pass the `…_ARN_KIND` constant for the entity.
pub fn label_namespace(kind: &str, name: &str) -> String {
    format!(
        "awswaf:{}:{kind}:{name}:",
        ::engine::emulator_account_id()
    )
}

/// Check one `Addresses` entry against the set's declared `IPAddressVersion`.
///
/// `IPSet$Addresses` documents the rule: "All addresses must be specified using
/// Classless Inter-Domain Routing (CIDR) notation. WAF supports all IPv4 and IPv6
/// CIDR ranges except for `/0`", and `IPAddressVersion` fixes which family the
/// entries belong to. `Err` carries the reason, for the refusal's message.
///
/// What the doc does NOT say is anything about the host bits below the prefix
/// length, so `10.0.0.1/8` is deliberately accepted and stored verbatim: the
/// documented rule is the notation, the family and the `/0` exclusion, and this
/// check goes no further than the rule it can cite.
pub fn validate_cidr(address: &str, ip_address_version: &str) -> Result<(), String> {
    let Some((literal, prefix)) = address.split_once('/') else {
        return Err(format!(
            "{address:?} is not in CIDR notation; expected <address>/<prefix length>"
        ));
    };
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{address:?} has a non-numeric CIDR prefix length"));
    }
    let Ok(prefix_len) = prefix.parse::<u32>() else {
        return Err(format!("{address:?} has an out-of-range CIDR prefix length"));
    };
    // `/0` is the one range WAF documents as unsupported, for either family.
    let (parsed, max_prefix) = match ip_address_version {
        "IPV4" => (
            literal.parse::<::std::net::Ipv4Addr>().is_ok(),
            32_u32,
        ),
        "IPV6" => (
            literal.parse::<::std::net::Ipv6Addr>().is_ok(),
            128_u32,
        ),
        other => {
            return Err(format!("{other:?} is not a known IPAddressVersion"));
        }
    };
    if !parsed {
        return Err(format!(
            "{address:?} is not an {ip_address_version} address in CIDR notation"
        ));
    }
    if prefix_len == 0 || prefix_len > max_prefix {
        return Err(format!(
            "{address:?} has prefix length /{prefix_len}; {ip_address_version} \
             CIDR ranges must be between /1 and /{max_prefix}"
        ));
    }
    Ok(())
}

/// The web ACL capacity units (WCUs) a rule list consumes.
///
/// `WebACL$Capacity` is "the web ACL capacity units (WCUs) currently being used by
/// this web ACL", so it is DERIVED from the stored rules rather than carried in
/// the request. The constants below are the base WCU costs the WAF Developer
/// Guide publishes for each rule statement in its rule-statement list; a logical
/// statement costs the sum of what it nests.
///
/// One cost is not a constant: a `RuleGroupReferenceStatement` costs the
/// referenced rule group's own `Capacity` — "the web ACL capacity units (WCUs)
/// required for this rule group", fixed when the group is created — so
/// `rule_group_capacities` carries `ARN` → `Capacity` for the scope being saved
/// into, as [`ComAmazonawsWafv2Awswaf20190729State::rule_group_capacities`] reads
/// it out of state. A reference naming a rule group this service does not hold is
/// refused on ingress by
/// [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`], so every
/// reference a stored rule list carries is in that map.
///
/// A `ManagedRuleGroupStatement` is priced the same way, from the published
/// capacity of the catalogued group it names ([`managed_rule_group`]): a managed
/// rule group's capacity is a property of the group, not of the request, and a
/// statement naming a group the catalogue does not list is refused on ingress by
/// the same resolution, so this function never has to price a statement whose cost
/// it cannot know and no caller is handed an understated `Capacity`.
///
/// Every term is added with SATURATING arithmetic: one of them — a referenced rule
/// group's declared `Capacity` — is caller-supplied and modelled `range 1..`, so a
/// rule list referencing enough expensive groups could drive the sum past
/// `i64::MAX`. Saturating at the modelled maximum answers a capacity that is simply
/// above any limit, where wrapping would answer a NEGATIVE capacity (which
/// `ConsumedCapacity` is `range 0..`) and overflow-checked arithmetic would panic on
/// a request.
///
/// What this reports decides a SAVE as well as a report: `CheckCapacity` publishes
/// it, and `CreateRuleGroup` refuses a rule list that needs more than the
/// `Capacity` the request declares ("WAF enforces this limit"). So a figure below
/// publishes an understated price, and a figure ABOVE the published cost of its
/// statement kind would refuse a save WAF accepts — re-price and re-read that
/// refusal together.
pub fn web_acl_capacity(rules: &[Rule], rule_group_capacities: &HashMap<String, i64>) -> i64 {
    rules.iter().fold(0_i64, |total, rule| {
        total.saturating_add(statement_capacity(
            &rule.statement,
            MAX_STATEMENT_NESTING_DEPTH,
            rule_group_capacities,
        ))
    })
}

/// The WCUs one `ByteMatchStatement` consumes.
///
/// The WCU table lives in the WAF Developer Guide, not in the Smithy model
/// (`WebACL$Capacity` only points at that guide), so where the recorded wire corpus
/// prices a statement it is the authority here: a recorded web ACL whose three
/// rules each hold ONE `ByteMatchStatement` — over a single field, with one `NONE`
/// text transformation and a `CONTAINS` positional constraint — reports `Capacity`
/// 30, so one such statement costs 10 rather than the 1 a plain single-field match
/// is often quoted at. Nothing available here prices the other positional
/// constraints separately, so every byte match costs this.
pub const BYTE_MATCH_WCU: i64 = 10;

/// The WCUs one `RateBasedStatement` consumes before anything it nests.
///
/// Pinned by the recorded corpus: a web ACL holding exactly one rate-based rule
/// that aggregates on `IP`, with no scope-down statement and no custom keys,
/// reports `Capacity` 2.
pub const RATE_BASED_WCU: i64 = 2;

/// The WCUs a `RateBasedStatement` adds when it aggregates on CUSTOM KEYS.
///
/// Aggregating on request components costs more than aggregating on the source IP,
/// because WAF has to extract and hash the components of every request:
/// `RateBasedStatement$AggregateKeyType` documents `CUSTOM_KEYS` as "aggregate the
/// request counts using one or more web request components as the aggregate keys",
/// specified in `CustomKeys`. Pinned by the recorded corpus: the same web ACL,
/// updated so its one rate-based rule aggregates on a custom key, reports
/// `Capacity` 32 — [`RATE_BASED_WCU`] plus this. The recording pins the cost of
/// aggregating on custom keys, not a price per key (`RateBasedStatementCustomKeys`
/// is `length 0..5`), so it is charged once for the list.
pub const RATE_BASED_CUSTOM_KEYS_WCU: i64 = 30;

/// The WCU cost of one statement. See [`web_acl_capacity`].
///
/// `depth_budget` bounds the descent exactly as in
/// [`statement_references_arn`]: a caller-supplied tree deeper than
/// [`MAX_STATEMENT_NESTING_DEPTH`] is refused on ingress, and this budget is the
/// second line of defence so no request can drive the recursion past it.
fn statement_capacity(
    statement: &Statement,
    depth_budget: u32,
    rule_group_capacities: &HashMap<String, i64>,
) -> i64 {
    if depth_budget == 0 {
        return 0;
    }
    // Every term is added with saturating arithmetic; see [`web_acl_capacity`].
    let mut capacity = 0_i64;
    if statement.byte_match_statement.is_some() {
        capacity = capacity.saturating_add(BYTE_MATCH_WCU);
    }
    // Base cost 1: size constraint, geo match, IP set reference, label match and
    // ASN match statements.
    for present in [
        statement.size_constraint_statement.is_some(),
        statement.geo_match_statement.is_some(),
        statement.ip_set_reference_statement.is_some(),
        statement.label_match_statement.is_some(),
        statement.asn_match_statement.is_some(),
    ] {
        if present {
            capacity = capacity.saturating_add(1);
        }
    }
    if statement.sqli_match_statement.is_some() {
        capacity = capacity.saturating_add(20);
    }
    if statement.xss_match_statement.is_some() {
        capacity = capacity.saturating_add(40);
    }
    if statement.regex_match_statement.is_some() {
        capacity = capacity.saturating_add(3);
    }
    if statement.regex_pattern_set_reference_statement.is_some() {
        capacity = capacity.saturating_add(25);
    }
    // A rate-based statement costs `RATE_BASED_WCU` on its own, plus
    // `RATE_BASED_CUSTOM_KEYS_WCU` when it aggregates on custom keys; every nesting
    // (logical operator or scope-down) costs what it nests, which
    // `nested_statements` enumerates.
    if let Some(rate) = statement.rate_based_statement.as_ref() {
        capacity = capacity.saturating_add(RATE_BASED_WCU);
        if !rate.custom_keys.as_deref().unwrap_or_default().is_empty() {
            capacity = capacity.saturating_add(RATE_BASED_CUSTOM_KEYS_WCU);
        }
    }
    // A rule group reference costs the referenced group's declared capacity, which
    // `rule_group_capacities` carries for every rule group of this scope. A
    // reference naming one this service does not hold never reaches here.
    if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
        capacity = capacity.saturating_add(
            rule_group_capacities
                .get(&reference.arn)
                .copied()
                .unwrap_or_default(),
        );
    }
    // A managed rule group reference costs the published capacity of the group it
    // names. A statement naming a group the catalogue does not list never reaches
    // here.
    if let Some(managed) = statement.managed_rule_group_statement.as_ref() {
        capacity = capacity.saturating_add(
            managed_rule_group(&managed.vendor_name, &managed.name)
                .map(|group| group.capacity)
                .unwrap_or_default(),
        );
    }
    nested_statements(statement)
        .into_iter()
        .fold(capacity, |total, nested| {
            total.saturating_add(statement_capacity(
                nested,
                depth_budget - 1,
                rule_group_capacities,
            ))
        })
}

/// The `Description` a WAFv2 entity reports when the save that created it supplied
/// none.
///
/// Every WAFv2 response that carries an entity, or one of its `…Summary`
/// projections, carries the `Description` member: the recorded wire responses show
/// `""` for an entity saved without a description, rather than the member being
/// omitted. So the empty string is what gets STORED, and `Get`, `Create` and
/// `List` all report it from state instead of each deciding whether to emit the
/// member. (`EntityDescription` is modelled `length 1..256`, which bounds what a
/// REQUEST may carry; the empty description a RESPONSE reports is recorded
/// behaviour.)
pub fn stored_description(requested: Option<String>) -> Option<String> {
    Some(requested.unwrap_or_default())
}

/// The DDoS-protection mode a web ACL carries when the save names none.
///
/// `OnSourceDDoSProtectionConfig$ALBLowReputationMode` documents
/// `ACTIVE_UNDER_DDOS` as "enabled by default whenever a web ACL is associated with
/// an Application Load Balancer", and the member is `required` inside the
/// structure, so the account default is a real stored value — not an absent
/// structure — and a read reports it.
pub const DEFAULT_ALB_LOW_REPUTATION_MODE: &str = "ACTIVE_UNDER_DDOS";

/// The evaluation window a rate-based statement carries when the save names none.
///
/// `RateBasedStatement$EvaluationWindowSec` documents "Valid settings are 60, 120,
/// 300, and 600" and "Default: `300` (5 minutes)". The member's modelled
/// zero-value default is `0`, which is not one of the valid settings, so a request
/// that omits it arrives here as `0` and WAF's documented default is what gets
/// stored.
pub const DEFAULT_EVALUATION_WINDOW_SEC: i64 = 300;

/// Materialise, in place, every default WAF applies inside a saved rule tree.
///
/// Walked with an explicit worklist, like [`validate_statement_nesting`]: the tree
/// is supplied by the request, so nothing here may consume stack in proportion to
/// its depth. A default is applied wherever the statement that carries it sits,
/// top level or nested.
pub fn apply_rule_defaults(rules: &mut [Rule]) {
    let mut pending: Vec<&mut Statement> = rules.iter_mut().map(|rule| &mut rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(rate) = statement
            .rate_based_statement
            .as_mut()
            .filter(|rate| rate.evaluation_window_sec == 0)
        {
            rate.evaluation_window_sec = DEFAULT_EVALUATION_WINDOW_SEC;
        }
        pending.extend(nested_statements_mut(statement));
    }
}

/// Materialise, on a web ACL about to be stored, every member WAF derives or
/// defaults server-side on a save.
///
/// One place, called by both `CreateWebACL` and `UpdateWebACL`, so a read cannot
/// see a web ACL that was created one way and updated another:
///
/// * `Description` — [`stored_description`].
/// * `Rules` — always a list, so a read of a rule-less web ACL answers `[]` rather
///   than omitting the member; `Rules` is "the Rule statements used to identify the
///   web requests that you want to manage", and a web ACL with none has an empty
///   collection of them, not an unknown one.
/// * `OnSourceDDoSProtectionConfig` — [`DEFAULT_ALB_LOW_REPUTATION_MODE`].
/// * `Capacity` — [`web_acl_capacity`] of the stored rules, which is why it is
///   computed AFTER the rule defaults are applied. `rule_group_capacities` prices
///   the rule-group references the tree carries; see [`web_acl_capacity`].
pub fn apply_web_acl_defaults(
    web_acl: &mut WebACL,
    rule_group_capacities: &HashMap<String, i64>,
) {
    web_acl.description = stored_description(web_acl.description.take());
    let mut rules = web_acl.rules.take().unwrap_or_default();
    apply_rule_defaults(&mut rules);
    web_acl.capacity = web_acl_capacity(&rules, rule_group_capacities);
    web_acl.rules = Some(rules);
    web_acl.on_source_d_do_s_protection_config = Some(
        web_acl
            .on_source_d_do_s_protection_config
            .take()
            .unwrap_or_else(|| OnSourceDDoSProtectionConfig {
                alb_low_reputation_mode: DEFAULT_ALB_LOW_REPUTATION_MODE.to_string(),
            }),
    );
}

/// The labels a rule group's rules ADD to matching requests, deduplicated and in
/// a stable order.
///
/// `RuleGroup$AvailableLabels` is "the labels that one or more rules in this rule
/// group add to matching web requests. These labels are defined in the
/// `RuleLabels` for a Rule", so it is DERIVED from the stored rules rather than
/// carried in the request. Each entry is the label as the rule specified it
/// (`LabelSummary$Name` is "an individual label specification"); the prefix every
/// one of them acquires at match time is reported separately, as
/// [`label_namespace`].
pub fn rule_group_available_labels(rules: &[Rule]) -> Vec<LabelSummary> {
    label_summaries(
        rules
            .iter()
            .flat_map(|rule| rule.rule_labels.as_deref().unwrap_or_default())
            .map(|label| label.name.as_str()),
    )
}

/// `names` as `LabelSummary` entries, deduplicated and in a stable order.
///
/// One projection for every list of labels this service reports —
/// [`rule_group_available_labels`] over a stored rule tree,
/// [`rule_group_consumed_labels`] over the label-match keys in one, and
/// [`ManagedRuleGroupDescriptor::available_labels`] over a published group's rules
/// — so the deduplication and the order are decided in one place whatever the
/// labels were read from.
fn label_summaries<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<LabelSummary> {
    let unique: ::std::collections::BTreeSet<&str> = names.into_iter().collect();
    unique
        .into_iter()
        .map(|name| LabelSummary {
            name: Some(name.to_string()),
        })
        .collect()
}

/// The labels a rule group's rules MATCH AGAINST, deduplicated and in a stable
/// order.
///
/// `RuleGroup$ConsumedLabels` is "the labels that one or more rules in this rule
/// group match against in label match statements. These labels are defined in a
/// `LabelMatchStatement` specification, in the Statement definition of a rule", so
/// it is derived from the stored rules the way [`rule_group_available_labels`] is —
/// and from the whole statement tree, because a `LabelMatchStatement` can sit
/// inside any nesting [`nested_statements`] enumerates.
pub fn rule_group_consumed_labels(rules: &[Rule]) -> Vec<LabelSummary> {
    let mut keys: Vec<&str> = Vec::new();
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if let Some(label_match) = statement.label_match_statement.as_ref() {
            keys.push(label_match.key.as_str());
        }
        pending.extend(nested_statements(statement));
    }
    label_summaries(keys)
}

/// Materialise, on a rule group about to be stored, every member WAF derives or
/// defaults server-side on a save.
///
/// The rule-group counterpart of [`apply_web_acl_defaults`], so a rule group reads
/// back the same way however it was last saved:
///
/// * `Description` — [`stored_description`].
/// * `Rules` — always a list, for the same reason the web ACL's is: a rule group
///   with no rules has an empty collection of them, not an unknown one.
/// * `AvailableLabels` / `ConsumedLabels` — derived from the stored rules by
///   [`rule_group_available_labels`] and [`rule_group_consumed_labels`], which is
///   why they are computed AFTER the rule defaults are applied.
///
/// `Capacity` is NOT derived here: unlike `WebACL$Capacity`, a rule group's is
/// "the web ACL capacity units (WCUs) required for this rule group", which the
/// create call declares and "you cannot change it after creation" — so the
/// declared value is what is stored and reported.
pub fn apply_rule_group_defaults(rule_group: &mut RuleGroup) {
    rule_group.description = stored_description(rule_group.description.take());
    let mut rules = rule_group.rules.take().unwrap_or_default();
    apply_rule_defaults(&mut rules);
    rule_group.available_labels = Some(rule_group_available_labels(&rules));
    rule_group.consumed_labels = Some(rule_group_consumed_labels(&rules));
    rule_group.rules = Some(rules);
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWafv2Awswaf20190729State>)
}

// ===========================================================================
// The WAFv2 request-validation layer
// ===========================================================================
//
// Two facts about how WAF refuses a bad parameter live here, and both are about
// the ANSWER rather than about the checking:
//
//   * WAF answers with its OWN modelled `WAFInvalidParameterException`, which the
//     model gives `Field`, `Parameter` and `Reason` members beside the message —
//     so a client can read WHICH input to correct instead of parsing prose.
//     [`invalid_parameter_exception`] builds it, and composes the message from
//     exactly those three the way WAF does.
//   * WAF resolves what a request REFERENCES before it judges the syntax of the
//     request's other members, and it reports a collection whose ELEMENTS are at
//     fault against the collection member's own path. Neither is something the
//     request boundary can do for a handler: the boundary runs every modelled
//     constraint first, and words an element failure at the element's indexed
//     path. An operation therefore hands the members concerned over with
//     `skip_validation` on its input struct (see `src/ops/types/`) and checks them
//     here, at the point in its own sequence where WAF checks them.
//
// NOTHING about constraint CHECKING is re-implemented. The constraints a member's
// `skip_validation` strips off its generated member schema are RE-DECLARED below
// in the framework's own representation — a [`TraitMap`] of
// `LengthTrait`/`PatternTrait`/`ElementConstraintsTrait`, the exact form
// `vendor/smithy-macros` would have attached — hung on a member schema of the same
// name, and the check itself is
// [`::aws_smithy_schema_server::constraints::validate`], the SAME walk the
// boundary runs. So the clause, the elision of a long value, the violation cap and
// the lowerCamel path stay the framework's, and a member refused here reads
// identically to one refused at the boundary.

/// The `ParameterExceptionField` value naming a resource ARN
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `RESOURCE_ARN` among its values).
pub const FIELD_RESOURCE_ARN: &str = "RESOURCE_ARN";

/// The `ParameterExceptionField` value naming a web ACL. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_WEB_ACL: &str = "WEB_ACL";

/// The `ParameterExceptionField` value naming a rule group. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE_GROUP: &str = "RULE_GROUP";

/// The `ParameterExceptionField` value naming a `RuleGroupReferenceStatement`. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_RULE_GROUP_REFERENCE_STATEMENT: &str = "RULE_GROUP_REFERENCE_STATEMENT";

/// The `ParameterExceptionField` value naming a managed rule group, as the enum
/// spells it (`MANAGED_RULE_SET`). See [`FIELD_RESOURCE_ARN`].
pub const FIELD_MANAGED_RULE_SET: &str = "MANAGED_RULE_SET";

/// The `ParameterExceptionField` value naming a managed-rule-group statement
/// (`MANAGED_RULE_SET_STATEMENT`, as the enum spells it). See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_MANAGED_RULE_SET_STATEMENT: &str = "MANAGED_RULE_SET_STATEMENT";

/// The `ParameterExceptionField` value naming a `Scope`. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_SCOPE_VALUE: &str = "SCOPE_VALUE";

/// The `ParameterExceptionField` value naming a rule statement. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_STATEMENT: &str = "STATEMENT";

/// The `ParameterExceptionField` value naming an IP address. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_IP_ADDRESS: &str = "IP_ADDRESS";

/// The `ParameterExceptionField` value naming a per-call entity limit. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_ENTITY_LIMIT: &str = "ENTITY_LIMIT";

/// The `ParameterExceptionField` value naming a token domain. See
/// [`FIELD_RESOURCE_ARN`].
pub const FIELD_TOKEN_DOMAIN: &str = "TOKEN_DOMAIN";

/// The `ErrorReason` WAF carries for an ARN it will not accept, as the recorded
/// WAF responses show it.
///
/// One sentence for the whole class — an ARN that is malformed, and an ARN that is
/// well formed but names nothing WAF can use for this parameter — because that is
/// what WAF answers for both: the `Parameter` member carries the rejected value,
/// so the reason does not have to distinguish them.
pub const ARN_NOT_VALID_REASON: &str = "The ARN isn't valid. A valid ARN begins with arn: \
     and includes other information separated by colons or slashes.";

/// The resource-kind segment a rule group's ARN carries
/// (`arn:…:wafv2:…:<global|regional>/rulegroup/<name>/<id>`, the form
/// [`resource_arn`] mints).
pub const RULE_GROUP_ARN_KIND: &str = "rulegroup";

/// The resource-kind segment a web ACL's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const WEB_ACL_ARN_KIND: &str = "webacl";

/// The resource-kind segment an IP set's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const IP_SET_ARN_KIND: &str = "ipset";

/// The resource-kind segment a regex pattern set's ARN carries. See
/// [`RULE_GROUP_ARN_KIND`].
pub const REGEX_PATTERN_SET_ARN_KIND: &str = "regexpatternset";

/// `WAFInvalidParameterException` as WAF returns it: the structured `Field`,
/// `Parameter` and `Reason` the model gives this one exception, and the message
/// WAF composes from exactly those three.
///
/// This is the one WAFv2 exception whose body describes the CALL (see the note
/// above the constant sentences at the top of this module): `Field` is "the
/// settings where the invalid parameter was found", `Parameter` is "the invalid
/// parameter that resulted in the exception" and `Reason` is "additional
/// information about the exception" (each member's own doc). The message is built
/// from the response's own three members and carries nothing else from the
/// request, so every refusal of this type reads alike whatever tripped it.
pub fn invalid_parameter_exception(
    field: &str,
    parameter: &str,
    reason: &str,
) -> WAFInvalidParameterException {
    WAFInvalidParameterException {
        message: Some(format!(
            "Error reason: {reason}, field: {field}, parameter: {parameter}"
        )),
        field: Some(field.to_string()),
        parameter: Some(parameter.to_string()),
        reason: Some(reason.to_string()),
    }
}

/// [`invalid_parameter_exception`] for an ARN WAF will not accept: `Field`
/// [`FIELD_RESOURCE_ARN`], `Parameter` the rejected ARN and `Reason`
/// [`ARN_NOT_VALID_REASON`].
pub fn invalid_arn_exception(arn: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(FIELD_RESOURCE_ARN, arn, ARN_NOT_VALID_REASON)
}

/// The `ErrorReason` WAF carries for a value it cannot decode as an API key, as the
/// recorded WAF responses show it.
pub const KEY_FORMAT_NOT_VALID_REASON: &str = "The parameter contains formatting \
     that is not valid.";

/// [`invalid_parameter_exception`] for an API key value WAF cannot decode: the
/// rejected value as `Parameter`, [`KEY_FORMAT_NOT_VALID_REASON`] as `Reason` and NO
/// `Field` — `ParameterExceptionField` declares no value naming an API key
/// (`smithy_query(query="shape",shape="ParameterExceptionField")`), so WAF leaves the
/// member out, which its message renders as `field: null`.
///
/// A value that does not decode to a key ([`is_api_key_shaped`]) is refused this way
/// by every operation that takes one, and is a different failure from a well-formed
/// key naming nothing stored, which is [`nonexistent_item_exception`].
pub fn invalid_key_format_exception(api_key: &str) -> WAFInvalidParameterException {
    WAFInvalidParameterException {
        message: Some(format!(
            "Error reason: {KEY_FORMAT_NOT_VALID_REASON}, field: null, parameter: {api_key}"
        )),
        field: None,
        parameter: Some(api_key.to_string()),
        reason: Some(KEY_FORMAT_NOT_VALID_REASON.to_string()),
    }
}

/// The largest page a paginated WAFv2 list answers with when the request names no
/// `Limit`: the upper bound of the model's `PaginationLimit`
/// (`smithy_query(query="shape",shape="PaginationLimit")` → `range 1..100`), which
/// every paginated operation of this service shares.
pub const MAX_PAGE_SIZE: i32 = 100;

/// How many entries one page holds, for a request whose `Limit` is `limit`.
///
/// Absent `Limit`, a page holds as many as [`MAX_PAGE_SIZE`] allows; a `Limit`
/// outside the model's `PaginationLimit` range is REFUSED rather than clamped, so
/// no caller gets a silently empty terminal page. One reading of that range for
/// every paginated operation, so a third one cannot grow a third copy of it.
///
/// `noun` is what the page holds (`"summaries"`, `"versions"`), for the refusal's
/// `Reason`; the refusal itself is the modelled `WAFInvalidParameterException`
/// carrying `Field` [`FIELD_ENTITY_LIMIT`] and the rejected value.
pub fn page_size(limit: Option<i32>, noun: &str) -> Result<usize, WAFInvalidParameterException> {
    let requested = match limit {
        None => MAX_PAGE_SIZE,
        Some(limit) if (1..=MAX_PAGE_SIZE).contains(&limit) => limit,
        Some(limit) => {
            return Err(invalid_parameter_exception(
                FIELD_ENTITY_LIMIT,
                &limit.to_string(),
                &format!("a page holds between 1 and {MAX_PAGE_SIZE} {noun}"),
            ));
        }
    };
    // The match above leaves `requested` in `1..=MAX_PAGE_SIZE`, so this cannot
    // fail: the conversion is here to state that invariant, not to hide a case.
    Ok(usize::try_from(requested).expect("a page size in 1..=MAX_PAGE_SIZE fits a usize"))
}

/// The resource-kind segment of a wafv2 resource ARN (`webacl`, `ipset`,
/// `rulegroup`, …), or `None` when `value` is not one at all.
///
/// The ARN layout is the one [`is_wafv2_resource_arn`] pins:
/// `arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`,
/// so the kind is the second slash-separated segment of the resource part.
pub fn arn_resource_kind(value: &str) -> Option<&str> {
    if !is_wafv2_resource_arn(value) {
        return None;
    }
    value.split(':').nth(5)?.split('/').nth(1)
}

/// This service's Smithy namespace, for the shadow member schemas below.
const NAMESPACE: &str = "com.amazonaws.wafv2";

/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$` — the `@pattern` shared by
/// `LockToken` and `EntityId`
/// (`smithy_query(query="shape",shape="LockToken,EntityId")`), the form
/// [`new_uuid`] mints.
const UUID_PATTERN: &str = r"^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$";

/// `^[\w\.\-/]+$` — `TokenDomain`'s `@pattern`
/// (`smithy_query(query="shape",shape="TokenDomain")`).
const TOKEN_DOMAIN_PATTERN: &str = r"^[\w\.\-/]+$";

/// `LockToken`: `@length(1..36)` plus [`UUID_PATTERN`].
static LOCK_TOKEN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(36))));
    traits.insert(Box::new(PatternTrait::new(UUID_PATTERN)));
    traits
});

/// `TokenDomain`: `@length(1..253)` plus [`TOKEN_DOMAIN_PATTERN`].
static TOKEN_DOMAIN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(253))));
    traits.insert(Box::new(PatternTrait::new(TOKEN_DOMAIN_PATTERN)));
    traits
});

/// `APIKeyTokenDomains`: `@length(min 1)` ELEMENTS, with `TokenDomain`'s own
/// constraints carried for each element exactly as codegen carries them
/// (`smithy_query(query="shape",shape="APIKeyTokenDomains")`).
static TOKEN_DOMAINS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), None)));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TOKEN_DOMAIN_TRAITS)));
    traits
});

/// `LockToken`, as every operation that takes an optimistic-lock token declares
/// it. The walk reads only a member schema's NAME (for the violation path) and its
/// trait map, so one declaration serves every such operation.
pub static M_LOCK_TOKEN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$LockToken",
        NAMESPACE,
        "LockToken",
    ),
    ShapeType::String,
    "LockToken",
    0,
)
.with_traits(&LOCK_TOKEN_TRAITS);

/// `TokenDomains`, which targets `APIKeyTokenDomains`.
pub static M_TOKEN_DOMAINS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput$TokenDomains",
        NAMESPACE,
        "TokenDomains",
    ),
    ShapeType::List,
    "TokenDomains",
    1,
)
.with_traits(&TOKEN_DOMAINS_TRAITS);

/// `TokenDomain`'s constraints as the aggregated clause lists them — every
/// constraint of the element shape, in one set, whichever of them an element
/// failed.
///
/// Spelled out because the framework's `Violation::requirement` wording is not
/// public: each sentence here is that function's own
/// (`Member must have length less than or equal to …`,
/// `Member must satisfy regular expression pattern: …`) and each bound is
/// `TokenDomain`'s, declared once in [`TOKEN_DOMAIN_TRAITS`] above. The ORDER is
/// the order WAF's request validator lists them: the length ceiling, the length
/// floor, then the pattern.
pub const TOKEN_DOMAIN_CONSTRAINTS: &[&str] = &[
    "Member must have length less than or equal to 253",
    "Member must have length greater than or equal to 1",
    r"Member must satisfy regular expression pattern: ^[\w\.\-/]+$",
];

/// The root of the shadow input the walk is driven over. Its member list is empty
/// on purpose: the walk visits what [`ConstrainedMembers`] WRITES, which is
/// whichever members the calling operation handed over, and a root struct schema
/// carries no member name so it contributes no path segment.
static CONSTRAINED_INPUT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.wafv2#ConstrainedInput",
        NAMESPACE,
        "ConstrainedInput",
    ),
    ShapeType::Structure,
    &[],
);

/// One value handed to the validating walk, written the way the codec writes that
/// shape so the walk builds the same path and runs the same checks.
pub enum ConstrainedValue<'a> {
    /// A string leaf.
    Text(&'a str),
    /// A list of strings; the walk names each element `<member>.<n>.member`.
    Texts(&'a [String]),
}

/// The members of ONE request whose constraints their operation handed over,
/// written for [`::aws_smithy_schema_server::constraints::validate`].
pub struct ConstrainedMembers<'a> {
    members: &'a [(&'static Schema, ConstrainedValue<'a>)],
}

impl SerializableStruct for ConstrainedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        for (schema, value) in self.members {
            match value {
                // A scalar list element is written against the COLLECTION member
                // schema, which is what `vendor/smithy-macros` emits and what lets
                // the walk tell the collection's element-count bound from the
                // elements' own bounds (the latter travel in
                // `ElementConstraintsTrait`).
                ConstrainedValue::Text(text) => s.write_string(schema, text)?,
                ConstrainedValue::Texts(texts) => s.write_list(schema, &|sl| {
                    for text in texts.iter() {
                        sl.write_string(schema, text)?;
                    }
                    Ok(())
                })?,
            }
        }
        Ok(())
    }
}

/// Run the framework's own constraint walk over the members an operation handed
/// over, and report every violation it found.
pub fn validate_members(members: &[(&'static Schema, ConstrainedValue<'_>)]) -> Vec<Violation> {
    validate(&ConstrainedMembers { members }, &CONSTRAINED_INPUT_SCHEMA)
}

/// The framework's own `ValidationException` envelope for `violations`, so a
/// member refused by a handler reads exactly like one refused at the boundary.
pub fn constraint_envelope(violations: &[Violation]) -> String {
    ::aws_smithy_schema_server::envelope(violations)
        .unwrap_or_else(|| "invalid request".to_string())
}

/// The envelope WAF's request validator answers for a collection of scalars,
/// given the violations the framework walk found in it — or `None` when it found
/// none.
///
/// WAF reports an ELEMENT that fails the element shape's own constraints against
/// the COLLECTION member's path, with the value rendered as the whole list and
/// every constraint of the element shape listed in one bracketed set: ONE clause
/// for the member, however many elements failed and however many constraints each
/// failed. A violation of the collection ITSELF — its element-count `@length` — is
/// a different constraint about a different value, so it keeps the framework's own
/// clause.
///
/// `member_path` is the collection member's path as the envelope names it (use
/// [`member_path`]), `elements` is the list as the request sent it, and
/// `element_constraints` is the element shape's constraint set (e.g.
/// [`TOKEN_DOMAIN_CONSTRAINTS`]).
pub fn collection_constraint_envelope(
    member_path: &str,
    elements: &[String],
    element_constraints: &[&str],
    violations: &[Violation],
) -> Option<String> {
    // The value every clause about this member echoes: the list, the way WAF
    // renders one.
    let rendered = format!("[{}]", echoed(&elements.join(", ")));
    let element_prefix = format!("{member_path}.");
    let mut clauses: Vec<String> = Vec::new();
    let mut element_clause_emitted = false;
    for violation in violations {
        // The collection ITSELF (its element-count `@length`): the framework's own
        // requirement wording, restated against the list as the value — the walk
        // renders a count there (`3 items`), and WAF renders the list.
        if violation.path == member_path {
            clauses.push(
                Violation {
                    path: violation.path.clone(),
                    kind: violation.kind.clone(),
                    value: Some(rendered.clone()),
                }
                .message(),
            );
            continue;
        }
        if !violation.path.starts_with(&element_prefix) {
            clauses.push(violation.message());
            continue;
        }
        // Every element failure of this member collapses into the one clause, so
        // it is emitted at the FIRST of them and skipped thereafter.
        if element_clause_emitted {
            continue;
        }
        element_clause_emitted = true;
        clauses.push(format!(
            "Value '{rendered}' at '{member_path}' failed to satisfy constraint: \
             Member must satisfy constraint: [{}]",
            element_constraints.join(", ")
        ));
    }
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// A member's path as a validation clause names it: the Smithy member name in
/// lowerCamel.
///
/// Delegates to the framework, which owns that casing rule, so a clause a handler
/// writes cannot spell a position differently from one the boundary writes.
pub fn member_path(smithy_name: &str) -> String {
    ::aws_smithy_schema_server::constraints::path::member(smithy_name)
}

/// The rejected value as it appears in a clause: itself when short, a
/// character-boundary-safe prefix with an elision when not.
///
/// The cap and its rendering are the framework's own (its `MAX_ECHOED_VALUE`, 256
/// bytes), re-stated here only because that helper is private. A caller controls
/// the length of what a clause echoes, so an unelided echo turns a large request
/// into an arbitrarily large response. It is applied to the RENDERING only: every
/// check runs on the value the caller actually sent.
fn echoed(value: &str) -> String {
    const MAX_ECHOED_VALUE: usize = 256;
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}
