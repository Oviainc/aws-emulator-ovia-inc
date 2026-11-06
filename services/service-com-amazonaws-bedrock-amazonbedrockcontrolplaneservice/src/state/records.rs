// SPDX-License-Identifier: CC0-1.0

//! The stored record of each guardrail, prompt router and inference profile a request
//! creates — the resources whose own lifecycle this file holds, beside the model, endpoint
//! and job records in `state/resource_records.rs`.

use super::*;

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

    /// Remove version `number` and release the idempotency tokens that named it, through
    /// the one rule [`drop_version`] states for both versioned resources of this service.
    pub fn remove_version(&mut self, number: u32) -> Option<GuardrailRevision> {
        drop_version(&mut self.versions, &mut self.version_tokens, number)
    }
}

/// Remove version `number` from `versions` and release the entries of `tokens` that named
/// it, handing back what was removed.
///
/// Here rather than on each record because this service has TWO versioned resources — a
/// guardrail and an Automated Reasoning policy — and each holds its snapshots and the
/// idempotency tokens that cut them in this same pair of maps, so dropping one version is
/// ONE rule: a token never outlives the version it minted.
pub(super) fn drop_version<V>(
    versions: &mut BTreeMap<u32, V>,
    tokens: &mut HashMap<String, u32>,
    number: u32,
) -> Option<V> {
    let removed = versions.remove(&number)?;
    tokens.retain(|_, minted| *minted != number);
    Some(removed)
}

/// One stored prompt router: the identity `CreatePromptRouter` minted, the
/// configuration that request carried, and the two instants a read reports.
///
/// `status` and `router_type` are `&'static str` for the same reason
/// [`GuardrailRevision::status`] is: both are single values this emulator mints rather
/// than anything a request chooses ([`PROMPT_ROUTER_AVAILABLE`],
/// [`PROMPT_ROUTER_TYPE_CUSTOM`]).
#[derive(Debug, Clone)]
pub struct PromptRouter {
    /// The `PromptRouterArn` this router is addressed by, minted by
    /// [`prompt_router_arn`].
    pub arn: String,
    /// `promptRouterName`, as the create request spelled it.
    pub name: String,
    pub description: Option<String>,
    /// `models` — the foundation models this router may route to, in request order.
    pub models: Vec<PromptRouterTargetModel>,
    pub fallback_model: PromptRouterTargetModel,
    pub routing_criteria: RoutingCriteria,
    pub tags: Vec<Tag>,
    /// `PromptRouterStatus`; [`PROMPT_ROUTER_AVAILABLE`] for every router minted here.
    pub status: &'static str,
    /// `PromptRouterType`; [`PROMPT_ROUTER_TYPE_CUSTOM`] for a router a caller created.
    pub router_type: &'static str,
    pub created_at: SystemTime,
    /// When the router last changed. Equal to [`PromptRouter::created_at`] until some
    /// operation changes the router; the model carries no `UpdatePromptRouter`, so no
    /// request moves it today.
    pub updated_at: SystemTime,
}

/// One stored application inference profile: the identity `CreateInferenceProfile`
/// minted, the model source that request named, and what a read reports.
///
/// `status` and `profile_type` are `&'static str` for the same reason
/// [`PromptRouter::status`] is: both are single values this emulator mints
/// ([`INFERENCE_PROFILE_ACTIVE`], [`INFERENCE_PROFILE_TYPE_APPLICATION`]) rather than
/// anything a request chooses.
#[derive(Debug, Clone)]
pub struct InferenceProfileRecord {
    /// The `InferenceProfileArn` this profile is addressed by, minted by
    /// [`application_inference_profile_arn`].
    pub arn: String,
    /// `inferenceProfileId` — the resource half of that ARN, so both spellings of an
    /// `InferenceProfileIdentifier` name this one record.
    pub id: String,
    /// `inferenceProfileName`, as the create request spelled it.
    pub name: String,
    pub description: Option<String>,
    /// The region the ARN carries, so a profile is only ever read in the region it was
    /// created in.
    pub region: String,
    /// `models` — the foundation models this profile tracks metrics and costs for,
    /// derived from the `modelSource` the create named.
    pub models: Vec<InferenceProfileModel>,
    pub tags: Vec<Tag>,
    /// `InferenceProfileStatus`; [`INFERENCE_PROFILE_ACTIVE`] for every profile minted
    /// here.
    pub status: &'static str,
    /// `InferenceProfileType`; [`INFERENCE_PROFILE_TYPE_APPLICATION`] for a profile a
    /// caller created.
    pub profile_type: &'static str,
    pub created_at: SystemTime,
    /// When the profile last changed. Equal to [`InferenceProfileRecord::created_at`]
    /// until some operation changes it; the model carries no `UpdateInferenceProfile`, so
    /// no request moves it today.
    pub updated_at: SystemTime,
}
