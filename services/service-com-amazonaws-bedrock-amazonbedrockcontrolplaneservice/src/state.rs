// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrock-amazonbedrockcontrolplaneservice service.
//!
//! Today this holds the guardrail resource: the control plane's self-contained
//! CRUD resource, owned by `ops/{create,get,list,update,delete}_guardrail.rs` and
//! `ops/create_guardrail_version.rs`. Every one of those handlers reads and writes
//! THIS struct through [`service_state`], so a guardrail created by one request is
//! the guardrail every later request sees.
//!
//! The record shapes, and the conversions between the `*PolicyConfig` shapes a
//! request carries and the `*Policy` shapes a response carries, live here rather
//! than in one operation's file because six operations share them (a file under
//! `src/ops/` is claimed by the build as an operation, so a shared helper cannot
//! live there).

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    GuardrailAutomatedReasoningPolicy, GuardrailAutomatedReasoningPolicyConfig,
    GuardrailContentFilter, GuardrailContentFilterConfig, GuardrailContentFiltersTier,
    GuardrailContentFiltersTierConfig, GuardrailContentPolicy, GuardrailContentPolicyConfig,
    GuardrailContextualGroundingFilter, GuardrailContextualGroundingFilterConfig,
    GuardrailContextualGroundingPolicy, GuardrailContextualGroundingPolicyConfig,
    GuardrailContextualGroundingTier, GuardrailContextualGroundingTierConfig,
    GuardrailCrossRegionConfig, GuardrailCrossRegionDetails, GuardrailManagedWords,
    GuardrailManagedWordsConfig, GuardrailPiiEntity, GuardrailPiiEntityConfig, GuardrailRegex,
    GuardrailRegexConfig, GuardrailSensitiveInformationPolicy,
    GuardrailSensitiveInformationPolicyConfig, GuardrailTopic, GuardrailTopicConfig,
    GuardrailTopicPolicy, GuardrailTopicPolicyConfig, GuardrailTopicsTier,
    GuardrailTopicsTierConfig, GuardrailWord, GuardrailWordConfig, GuardrailWordPolicy,
    GuardrailWordPolicyConfig, Tag,
};

/// The working version every guardrail always has, as `GuardrailDraftVersion`
/// (`^DRAFT$`) spells it.
pub const DRAFT_VERSION: &str = "DRAFT";

/// `GuardrailStatus` — a guardrail that is serving. The status every revision this
/// emulator mints carries, from the call that mints it onwards.
///
/// `CreateGuardrail`, `UpdateGuardrail` and `CreateGuardrailVersion` return no status
/// member at all, and the first read that follows any of them reports `READY`, so a
/// guardrail is never observed in a non-terminal status: there is no transition for the
/// clock to owe, and nothing asynchronous for this emulator to wait on. The other
/// `GuardrailStatus` values (`CREATING`, `UPDATING`, `VERSIONING`, `DELETING`, `FAILED`)
/// name states no call here produces, so none of them is minted.
pub const STATUS_READY: &str = "READY";

/// `GuardrailContentFiltersTierName` / `GuardrailTopicsTierName` /
/// `GuardrailContextualGroundingTierName` — the tier a policy block reports when the
/// request configured none.
///
/// All three enums are `[CLASSIC, STANDARD]`, and the model documents `STANDARD` as the
/// opt-in tier that "requires that your guardrail use cross-Region inference" while
/// `CLASSIC` "provides established guardrails functionality" — so `CLASSIC` is the tier a
/// guardrail is on until a request names the other one.
pub const TIER_CLASSIC: &str = "CLASSIC";

/// The most guardrails this account may hold at once, past which a create is
/// refused with `ServiceQuotaExceededException`.
///
/// The Amazon Bedrock service quota "Guardrails per account" has a default of 100
/// (adjustable); this emulator enforces that default, so the store cannot grow
/// without a server-side bound no matter how many creates a caller issues.
pub const MAX_GUARDRAILS_PER_ACCOUNT: usize = 100;

/// The most numbered versions one guardrail may hold at once, past which cutting
/// another is refused with `ServiceQuotaExceededException`.
///
/// The Amazon Bedrock service quota "Versions per guardrail" has a default of 20
/// (adjustable); this emulator enforces that default, so the snapshots one
/// guardrail accumulates are bounded too.
pub const MAX_VERSIONS_PER_GUARDRAIL: usize = 20;

/// The largest version number `GuardrailNumericalVersion` (`^[1-9][0-9]{0,7}$`) can
/// spell. Version numbers are never reused, so a guardrail that has cut and deleted
/// this many versions can mint no more and `CreateGuardrailVersion` refuses rather
/// than minting a number the model does not admit.
pub const MAX_VERSION_NUMBER: u32 = 99_999_999;

/// The number of base-36 digits each half of a minted guardrail id carries.
///
/// `GuardrailId` is `^[a-z0-9]+$` with length at most 64; base 36 over
/// `0-9a-z` is exactly that alphabet.
const ID_HALF_WIDTH: usize = 6;

/// One immutable-or-working configuration of a guardrail: the DRAFT, or a
/// numbered version snapshotted from it.
#[derive(Debug, Clone)]
pub struct GuardrailRevision {
    /// `DRAFT`, or the decimal version number (`^[1-9][0-9]{0,7}$`).
    pub version: String,
    /// This revision's own description (the DRAFT's, or the one passed to
    /// `CreateGuardrailVersion`).
    pub description: Option<String>,
    pub blocked_input_messaging: String,
    pub blocked_outputs_messaging: String,
    pub topic_policy: Option<GuardrailTopicPolicy>,
    pub content_policy: Option<GuardrailContentPolicy>,
    pub word_policy: Option<GuardrailWordPolicy>,
    pub sensitive_information_policy: Option<GuardrailSensitiveInformationPolicy>,
    pub contextual_grounding_policy: Option<GuardrailContextualGroundingPolicy>,
    pub automated_reasoning_policy: Option<GuardrailAutomatedReasoningPolicy>,
    pub cross_region_details: Option<GuardrailCrossRegionDetails>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// This revision's `GuardrailStatus`. [`STATUS_READY`] for every revision this
    /// emulator mints; see that constant for why no other value is reachable.
    pub status: &'static str,
    /// `statusReasons` — why the guardrail failed to be created, updated, versioned or
    /// deleted. Empty for a revision that is not `FAILED`, which is every revision here,
    /// and reported as the empty list rather than omitted.
    pub status_reasons: Vec<String>,
    /// `failureRecommendations` — what to do before retrying. Empty alongside
    /// [`GuardrailRevision::status_reasons`], and reported the same way.
    pub failure_recommendations: Vec<String>,
}

impl GuardrailRevision {
    /// A revision carrying only the two required messaging strings, created at `now`.
    pub fn new(
        version: impl Into<String>,
        blocked_input_messaging: String,
        blocked_outputs_messaging: String,
        now: SystemTime,
    ) -> Self {
        Self {
            version: version.into(),
            description: None,
            blocked_input_messaging,
            blocked_outputs_messaging,
            topic_policy: None,
            content_policy: None,
            word_policy: None,
            sensitive_information_policy: None,
            contextual_grounding_policy: None,
            automated_reasoning_policy: None,
            cross_region_details: None,
            created_at: now,
            updated_at: now,
            status: STATUS_READY,
            status_reasons: Vec::new(),
            failure_recommendations: Vec::new(),
        }
    }

    /// Replace this revision's whole policy set with `configs`, converted to the
    /// response shapes. A config the request omits CLEARS that policy: a
    /// guardrail's policy set is replaced, never merged.
    pub fn set_policies(&mut self, configs: GuardrailPolicyConfigs, region: &str) {
        self.topic_policy = configs.topic.as_ref().map(topic_policy_of);
        self.content_policy = configs.content.as_ref().map(content_policy_of);
        self.word_policy = configs.word.as_ref().map(word_policy_of);
        self.sensitive_information_policy = configs
            .sensitive_information
            .as_ref()
            .map(sensitive_information_policy_of);
        self.contextual_grounding_policy = configs
            .contextual_grounding
            .as_ref()
            .map(contextual_grounding_policy_of);
        self.automated_reasoning_policy = configs
            .automated_reasoning
            .as_ref()
            .map(automated_reasoning_policy_of);
        self.cross_region_details = configs
            .cross_region
            .as_ref()
            .and_then(|config| cross_region_details_of(config, region));
    }
}

/// The policy configuration a `CreateGuardrail` or `UpdateGuardrail` request
/// carries, lifted out of those two request shapes so one conversion serves both.
#[derive(Debug, Default)]
pub struct GuardrailPolicyConfigs {
    pub topic: Option<GuardrailTopicPolicyConfig>,
    pub content: Option<GuardrailContentPolicyConfig>,
    pub word: Option<GuardrailWordPolicyConfig>,
    pub sensitive_information: Option<GuardrailSensitiveInformationPolicyConfig>,
    pub contextual_grounding: Option<GuardrailContextualGroundingPolicyConfig>,
    pub automated_reasoning: Option<GuardrailAutomatedReasoningPolicyConfig>,
    pub cross_region: Option<GuardrailCrossRegionConfig>,
}

/// A stored guardrail: the identity that never changes, plus its DRAFT and every
/// numbered version snapshotted from it.
#[derive(Debug, Clone)]
pub struct Guardrail {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub kms_key_arn: Option<String>,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub draft: GuardrailRevision,
    pub versions: BTreeMap<u32, GuardrailRevision>,
    /// `CreateGuardrailVersion` idempotency: `clientRequestToken` -> the version
    /// number that token cut.
    ///
    /// It lives INSIDE the record, and an entry is dropped when the version it
    /// names is deleted, so the map holds at most one entry per live version
    /// (bounded by [`MAX_VERSIONS_PER_GUARDRAIL`]) and dies with the guardrail by
    /// construction.
    pub version_tokens: HashMap<String, u32>,
    /// The number `CreateGuardrailVersion` will mint next, from `1` upwards.
    pub next_version: u32,
}

impl Guardrail {
    /// The revision a `guardrailVersion` names. An absent version, and the literal
    /// `DRAFT`, both name the DRAFT; anything else must be a stored number.
    pub fn revision(&self, version: Option<&str>) -> Option<&GuardrailRevision> {
        match version {
            None => Some(&self.draft),
            Some(DRAFT_VERSION) => Some(&self.draft),
            Some(number) => version_number(number).and_then(|n| self.versions.get(&n)),
        }
    }

    /// Every revision of this guardrail in list order: the DRAFT, then the
    /// numbered versions ascending.
    pub fn revisions(&self) -> impl Iterator<Item = &GuardrailRevision> {
        std::iter::once(&self.draft).chain(self.versions.values())
    }

    /// Remove version `number` and release the idempotency tokens that named it, so
    /// a token never outlives the version it minted.
    pub fn remove_version(&mut self, number: u32) -> Option<GuardrailRevision> {
        let removed = self.versions.remove(&number)?;
        self.version_tokens.retain(|_, minted| *minted != number);
        Some(removed)
    }
}

/// Per-service state for com-amazonaws-bedrock-amazonbedrockcontrolplaneservice. A handler reaches it through [`service_state`].
///
/// Every map here is bounded, and each bound holds by construction rather than by
/// convention:
///
/// * `guardrails` and `guardrail_names` hold one entry per live guardrail, capped at
///   [`MAX_GUARDRAILS_PER_ACCOUNT`] by the quota `CreateGuardrail` enforces.
/// * each guardrail's `versions` holds one entry per live version, capped at
///   [`MAX_VERSIONS_PER_GUARDRAIL`] by the quota `CreateGuardrailVersion` enforces.
/// * `create_tokens` records an entry only for a create that was ADMITTED (a refused
///   create records nothing), every entry names a live guardrail, and the entries
///   naming a guardrail are dropped when it is deleted — so it holds at most one
///   entry per live guardrail.
/// * each guardrail's [`Guardrail::version_tokens`] is bounded the same way against
///   its live versions, including when a SINGLE version is deleted
///   ([`Guardrail::remove_version`]).
///
/// So the whole store is bounded by the live resources the quotas admit, and no
/// sequence of requests — including create/delete cycles with fresh idempotency
/// tokens — can grow it past them.
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState {
    /// Every stored guardrail, keyed by its `guardrailId`.
    pub guardrails: HashMap<String, Guardrail>,
    /// `name` -> `guardrailId`, so a duplicate name is a `ConflictException`.
    pub guardrail_names: HashMap<String, String>,
    /// `CreateGuardrail` idempotency: `clientRequestToken` -> the `guardrailId`
    /// that token minted. Scoped to that one operation, so a token replayed
    /// against another operation is an independent request.
    pub create_tokens: HashMap<String, String>,
    /// How many guardrail ids this store has minted; the uniqueness half of a
    /// minted id.
    pub minted_guardrails: u64,
}

impl ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState {
    /// The guardrail a `guardrailIdentifier` (bare id or full ARN) names.
    pub fn guardrail(&self, identifier: &str) -> Option<&Guardrail> {
        self.guardrails.get(guardrail_id_of(identifier))
    }

    /// The guardrail a `guardrailIdentifier` names, for mutation.
    pub fn guardrail_mut(&mut self, identifier: &str) -> Option<&mut Guardrail> {
        self.guardrails.get_mut(guardrail_id_of(identifier))
    }

    /// Mint a fresh `guardrailId`: `^[a-z0-9]+$`, unique within this store.
    ///
    /// The leading half is this store's mint count, so two ids can never collide;
    /// the trailing half is drawn from the creation instant, so an id is not a
    /// guessable sequence number.
    pub fn mint_guardrail_id(&mut self, now: SystemTime) -> String {
        self.minted_guardrails += 1;
        let ordinal = self.minted_guardrails;
        let nanos = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos().into());
        format!(
            "{}{}",
            base36(ordinal, ID_HALF_WIDTH),
            base36(nanos, ID_HALF_WIDTH)
        )
    }
}

/// The `guardrailId` inside a `guardrailIdentifier`: the bare id, or the resource
/// half of a `.../guardrail/<id>` ARN.
pub fn guardrail_id_of(identifier: &str) -> &str {
    match identifier.rsplit_once(":guardrail/") {
        Some((_, id)) => id,
        None => identifier,
    }
}

/// The ARN of a guardrail in `region`:
/// `arn:aws:bedrock:<region>:<account>:guardrail/<id>`.
pub fn guardrail_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("guardrail/{id}"))
}

/// A `GuardrailVersion`/`GuardrailNumericalVersion` as a number, or `None` when it
/// is not a numbered version.
pub fn version_number(version: &str) -> Option<u32> {
    if version.is_empty() || version.starts_with('0') {
        return None;
    }
    version.parse::<u32>().ok()
}

/// The AWS region this request was signed for, defaulting to `us-east-1` for an
/// unsigned one, and folded back to the default when it is not region-shaped
/// (`GuardrailArn` admits `[a-z0-9-]{1,20}` there).
pub fn request_region(parts: &http::request::Parts) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    let region = ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region);
    match region {
        Some(region)
            if (1..=20).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            region
        }
        _ => DEFAULT_REGION.to_string(),
    }
}

// =============================================================================
// This service's own check of a caller-supplied ARN
// =============================================================================
//
// For SOME of its resources, Amazon Bedrock reads an ARN a caller submits ITSELF, ahead
// of the constraints the model declares for the member carrying it, and answers its own
// short sentence for one it will not accept — never the `N validation error(s) detected:
// Value '…' at '…' failed to satisfy constraint: …` envelope a constraint violation
// words. The two sentences below, and which resources they cover, are recorded in
// `simparity-wire-aws-v1`; which of them a request gets is decided here, and only here,
// so every member carrying an ARN of those resources answers alike.

/// This service's own ARN namespace: the third field of every ARN it owns.
///
/// `aws.api#service$arnNamespace` on this model is `bedrock`
/// (`smithy_query(query="service_info")`), so an ARN whose vendor field is another
/// service's — an IAM role, a KMS key, an S3 bucket — names no resource of this service
/// and is left to the constraints the model declares for it.
pub const ARN_NAMESPACE: &str = "bedrock";

/// What this service answers for an ARN of a resource it validates itself that names a
/// region other than the one the request addresses.
///
/// RECORDED behaviour, not a modelled rule: the model declares no per-region ARN check,
/// and the corpus records Amazon Bedrock answering 400 `ValidationException` with exactly
/// this sentence for an ARN of one of the resources in
/// [`SELF_VALIDATED_RESOURCE_TYPES`] from another region. A CONSTANT, interpolating no
/// part of the request: it reports that the ARN is not usable here, and nothing about
/// which member carried it or which expression that member declares.
pub const ARN_INVALID_FOR_SERVICE_REGION: &str =
    "The provided ARN is invalid for the service region";

/// The ARN resource types this service validates ITSELF — the ones the two sentences
/// above cover.
///
/// A CATALOG, read off the corpus, because nothing in the model marks the difference and
/// the corpus records BOTH answers for ARNs of the same shape and the same foreign
/// region:
///
/// * these four resources answer this service's own sentences —
///   `custom-model` (`CreateCustomModelDeployment.modelArn`,
///   `CreateModelCopyJob.sourceModelArn`), `model-copy-job` (`GetModelCopyJob.jobArn`),
///   `model-invocation-job` (`Get`/`StopModelInvocationJob.jobIdentifier`) and
///   `provisioned-model` (`Get`/`Update`/`DeleteProvisionedModelThroughput
///   .provisionedModelId`);
/// * an `advanced-prompt-optimization-job` and an `evaluation-job` ARN from the SAME
///   foreign region, submitted by the same probe generator, are refused with the
///   framework's constraint envelope instead — recorded in
///   `issues/…-bedrock-validation-error-messages-render-model-regex-with.md`, which is
///   why this is a catalog of the resources bedrock checks itself rather than a rule over
///   every ARN. A resource type NOT listed here keeps the refusal the model's own
///   constraints word, which is what that recording shows.
///
/// Matched on the ARN's resource TYPE rather than on an operation or a member, so every
/// member of this service that carries one of these ARNs — the ones the recorded causes
/// name and the ones they do not (`GetCustomModel.modelIdentifier`,
/// `CreateProvisionedModelThroughput.modelId`, …) — is read the same way.
pub const SELF_VALIDATED_RESOURCE_TYPES: &[&str] = &[
    "custom-model",
    "model-copy-job",
    "model-invocation-job",
    "provisioned-model",
];

/// A caller-supplied ARN this service validates itself: one of its own namespace naming a
/// resource type in [`SELF_VALIDATED_RESOURCE_TYPES`], as the parsed ARN and the type it
/// names.
///
/// The parse is `service_common::arn`'s one parse — six fields, the first literally
/// `arn`, a non-empty partition, vendor and resource, the vendor compared exactly — so no
/// two services disagree about what an ARN is or which one is theirs.
fn self_validated_arn(value: &str) -> Option<(::service_common::arn::Arn<'_>, &'static str)> {
    let arn = ::service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    let resource_type = arn.resource_type();
    SELF_VALIDATED_RESOURCE_TYPES
        .iter()
        .find(|listed| **listed == resource_type)
        .map(|listed| (arn, *listed))
}

/// Whether `value` is an ARN of this service's own namespace naming a region other than
/// `region` — the region the request addresses, as
/// [`request_region`] reads it off the request's SigV4 credential scope.
///
/// Only an ARN of a resource this service validates itself
/// ([`SELF_VALIDATED_RESOURCE_TYPES`]), and only one whose region field is NOT empty: an
/// empty region names none to disagree with, which is how a partition-global resource is
/// written.
#[must_use]
pub fn arn_names_another_region(value: &str, region: &str) -> bool {
    self_validated_arn(value)
        .is_some_and(|(arn, _)| !arn.region.is_empty() && arn.region != region)
}

/// What this service answers for an ARN of its own namespace that is not a well-formed
/// ARN of the resource type IT names: `Invalid <resource type> ARN format`, the type
/// spelled in words.
///
/// The corpus records `Invalid custom model ARN format` for a `custom-model` ARN whose
/// resource id is not one `CustomModelArn` admits, so the sentence is built from the
/// resource type the ARN itself carries rather than from the member or the member's
/// expression — a `model-copy-job` ARN reports the model copy job.
///
/// `None` for an ARN of a resource this service does not validate itself, and for a value
/// that is no ARN of its namespace at all: those are values this check has nothing to say
/// about, so the refusal the model's own constraints word stands.
#[must_use]
pub fn invalid_arn_format_message(value: &str) -> Option<String> {
    let (_, resource_type) = self_validated_arn(value)?;
    // An ARN resource type is a hyphenated token (`custom-model`,
    // `model-invocation-job`); the recorded sentence spells it as words.
    let words = resource_type.replace('-', " ");
    Some(format!("Invalid {words} ARN format"))
}

/// A `SystemTime` as the wire timestamp type the response shapes carry.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The `kmsKeyArn` a response reports for a request's `kmsKeyId`.
///
/// `kmsKeyId` admits a key ARN, a bare key id, or an alias, while `KmsKeyArn`
/// admits only `arn:aws:kms:<region>:<account>:key/<key id>`. A key ARN passes
/// through and a bare key id is completed into one; an ALIAS resolves to a key id
/// only inside KMS, so this reports no ARN for it rather than inventing one.
pub fn kms_key_arn(kms_key_id: Option<&str>, region: &str) -> Option<String> {
    let key_id = kms_key_id?;
    if key_id.starts_with("arn:") {
        return key_id.contains(":key/").then(|| key_id.to_string());
    }
    let is_key_id = key_id.len() == 36
        && key_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    is_key_id.then(|| ::engine::emulator_arn("kms", region, &format!("key/{key_id}")))
}

/// base-36 (`0-9a-z`) rendering of `value`, left-padded with `0` to `width`
/// digits.
fn base36(mut value: u64, width: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::with_capacity(width);
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// How a policy block's members are reported, which both conversions below and the
/// response construction in `ops/get_guardrail.rs` follow:
///
/// * A LIST member whose `@length` admits zero entries is always PRESENT — the empty list
///   when the request configured none — because the wire form of "nothing configured" for
///   such a member is `[]`, not an absent key. A list whose `@length` has `min: 1`
///   (`GuardrailContentPolicy$filters`, `GuardrailWordPolicy$words`,
///   `GuardrailSensitiveInformationPolicy$piiEntities`,
///   `GuardrailContextualGroundingPolicy$filters`) cannot be reported empty, so it stays
///   absent until the request configures it.
/// * A TIER member is always present on a block that models one, carrying the tier the
///   request named or [`TIER_CLASSIC`] when it named none.
fn topic_policy_of(config: &GuardrailTopicPolicyConfig) -> GuardrailTopicPolicy {
    GuardrailTopicPolicy {
        topics: config.topics_config.iter().map(topic_of).collect(),
        tier: Some(GuardrailTopicsTier {
            tier_name: config
                .tier_config
                .as_ref()
                .map_or_else(
                    || TIER_CLASSIC.to_string(),
                    |tier: &GuardrailTopicsTierConfig| tier.tier_name.clone(),
                ),
        }),
    }
}

fn topic_of(config: &GuardrailTopicConfig) -> GuardrailTopic {
    GuardrailTopic {
        name: config.name.clone(),
        definition: config.definition.clone(),
        // `GuardrailTopicExamples` is `@length(min: 0)`, so a topic with no examples
        // carries the empty list.
        examples: Some(config.examples.clone().unwrap_or_default()),
        r#type: Some(config.r#type.clone()),
        input_action: config.input_action.clone(),
        output_action: config.output_action.clone(),
        input_enabled: config.input_enabled,
        output_enabled: config.output_enabled,
    }
}

fn content_policy_of(config: &GuardrailContentPolicyConfig) -> GuardrailContentPolicy {
    GuardrailContentPolicy {
        filters: Some(
            config
                .filters_config
                .iter()
                .map(content_filter_of)
                .collect(),
        ),
        tier: Some(GuardrailContentFiltersTier {
            tier_name: config.tier_config.as_ref().map_or_else(
                || TIER_CLASSIC.to_string(),
                |tier: &GuardrailContentFiltersTierConfig| tier.tier_name.clone(),
            ),
        }),
    }
}

fn content_filter_of(config: &GuardrailContentFilterConfig) -> GuardrailContentFilter {
    GuardrailContentFilter {
        r#type: config.r#type.clone(),
        input_strength: config.input_strength.clone(),
        output_strength: config.output_strength.clone(),
        input_modalities: config.input_modalities.clone(),
        output_modalities: config.output_modalities.clone(),
        input_action: config.input_action.clone(),
        output_action: config.output_action.clone(),
        input_enabled: config.input_enabled,
        output_enabled: config.output_enabled,
    }
}

fn word_policy_of(config: &GuardrailWordPolicyConfig) -> GuardrailWordPolicy {
    GuardrailWordPolicy {
        words: config.words_config.as_ref().map(|words| {
            words
                .iter()
                .map(|word: &GuardrailWordConfig| GuardrailWord {
                    text: word.text.clone(),
                    input_action: word.input_action.clone(),
                    output_action: word.output_action.clone(),
                    input_enabled: word.input_enabled,
                    output_enabled: word.output_enabled,
                })
                .collect()
        }),
        managed_word_lists: Some(
            config
                .managed_word_lists_config
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|list: &GuardrailManagedWordsConfig| GuardrailManagedWords {
                    r#type: list.r#type.clone(),
                    input_action: list.input_action.clone(),
                    output_action: list.output_action.clone(),
                    input_enabled: list.input_enabled,
                    output_enabled: list.output_enabled,
                })
                .collect(),
        ),
    }
}

fn sensitive_information_policy_of(
    config: &GuardrailSensitiveInformationPolicyConfig,
) -> GuardrailSensitiveInformationPolicy {
    GuardrailSensitiveInformationPolicy {
        pii_entities: config.pii_entities_config.as_ref().map(|entities| {
            entities
                .iter()
                .map(|entity: &GuardrailPiiEntityConfig| GuardrailPiiEntity {
                    r#type: entity.r#type.clone(),
                    action: entity.action.clone(),
                    input_action: entity.input_action.clone(),
                    output_action: entity.output_action.clone(),
                    input_enabled: entity.input_enabled,
                    output_enabled: entity.output_enabled,
                })
                .collect()
        }),
        regexes: Some(
            config
                .regexes_config
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|regex: &GuardrailRegexConfig| GuardrailRegex {
                    name: regex.name.clone(),
                    description: regex.description.clone(),
                    pattern: regex.pattern.clone(),
                    action: regex.action.clone(),
                    input_action: regex.input_action.clone(),
                    output_action: regex.output_action.clone(),
                    input_enabled: regex.input_enabled,
                    output_enabled: regex.output_enabled,
                })
                .collect(),
        ),
    }
}

fn contextual_grounding_policy_of(
    config: &GuardrailContextualGroundingPolicyConfig,
) -> GuardrailContextualGroundingPolicy {
    GuardrailContextualGroundingPolicy {
        filters: config
            .filters_config
            .iter()
            .map(
                |filter: &GuardrailContextualGroundingFilterConfig| {
                    GuardrailContextualGroundingFilter {
                        r#type: filter.r#type.clone(),
                        threshold: filter.threshold,
                        action: filter.action.clone(),
                        enabled: filter.enabled,
                    }
                },
            )
            .collect(),
        tier: Some(GuardrailContextualGroundingTier {
            tier_name: config.tier_config.as_ref().map_or_else(
                || TIER_CLASSIC.to_string(),
                |tier: &GuardrailContextualGroundingTierConfig| tier.tier_name.clone(),
            ),
        }),
    }
}

fn automated_reasoning_policy_of(
    config: &GuardrailAutomatedReasoningPolicyConfig,
) -> GuardrailAutomatedReasoningPolicy {
    GuardrailAutomatedReasoningPolicy {
        policies: config.policies.clone(),
        confidence_threshold: config.confidence_threshold,
    }
}

/// The cross-region details for a `guardrailProfileIdentifier`, which may be a
/// profile id or a profile ARN.
///
/// `GuardrailCrossRegionGuardrailProfileId` is `<prefix>.guardrail.v<n>`; an
/// identifier that is not one of those names no profile this emulator can report,
/// so it reports none rather than inventing a profile.
fn cross_region_details_of(
    config: &GuardrailCrossRegionConfig,
    region: &str,
) -> Option<GuardrailCrossRegionDetails> {
    let identifier = config.guardrail_profile_identifier.as_str();
    let profile_id = match identifier.rsplit_once(":guardrail-profile/") {
        Some((_, id)) => id,
        None => identifier,
    };
    if !profile_id.contains(".guardrail.v") {
        return None;
    }
    Some(GuardrailCrossRegionDetails {
        guardrail_profile_id: Some(profile_id.to_string()),
        guardrail_profile_arn: Some(::engine::emulator_arn(
            "bedrock",
            region,
            &format!("guardrail-profile/{profile_id}"),
        )),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState>)
}
