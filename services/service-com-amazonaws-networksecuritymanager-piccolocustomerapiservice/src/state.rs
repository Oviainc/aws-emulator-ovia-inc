// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networksecuritymanager-piccolocustomerapiservice service.
//!
//! The Policy lifecycle (`CreatePolicy`, `GetPolicy`, `ListPolicies`, `UpdatePolicy`,
//! `DeletePolicy`) is backed by [`ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState::policies`],
//! the single source of truth those five handlers read and write. The record, the
//! id/token minting, the `policyIdentifier` resolution and the `nextToken` codec live
//! here rather than in any one `src/ops/*.rs`, so all five operations agree on one copy
//! of each.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AssociatedTemplateOrRule, PolicyConfiguration, PolicySummary, TemplateOrRuleReference,
};

/// This service's ARN namespace (`awsService.arnNamespace` / `aws.auth#sigv4` name in
/// the model), i.e. the service segment of every ARN it mints.
pub const ARN_NAMESPACE: &str = "network-security-manager";

/// The region an ARN carries when the request has no SigV4 credential scope to read one
/// from (an unsigned call). Matches the region the emulator's own shared config names.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EntityStatus` for a published policy — the model: "DRAFT (unpublished, editable) or
/// ACTIVE (published, in use)".
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// `EntityStatus` for an unpublished policy. See [`STATUS_ACTIVE`].
pub const STATUS_DRAFT: &str = "DRAFT";

/// `PolicyFirewallType` value whose policies carry no template/rule associations.
pub const FIREWALL_TYPE_SHIELD_ADVANCED: &str = "SHIELD_ADVANCED";

/// `ServiceResourceType` naming a policy, for the `resourceType` member the modelled
/// `ConflictException` / `ResourceNotFoundException` shapes carry.
pub const RESOURCE_TYPE_POLICY: &str = "Policy";

/// The largest page `ListPolicies` can answer with, and so the page size used when the
/// caller sends no `maxResults`: `PolicySummaryList` is modelled `@length(max: 100)`, so
/// a larger page could not be serialized at all.
pub const MAX_POLICY_PAGE: usize = 100;

/// Version tag of the opaque `nextToken` rendering, so a token minted by an older
/// emulator is rejected as invalid rather than silently misread.
const NEXT_TOKEN_VERSION: &str = "1";

/// The most policies this emulator stores, after which `CreatePolicy` answers with the
/// modelled `ServiceQuotaExceededException` (HTTP 402, "The request would exceed a
/// service quota") instead of growing [`Self::policies`] for as long as a caller keeps
/// asking. The number is a CHOSEN emulator capacity, not a quota this service documents:
/// what matters is that the store has a ceiling and that reaching it is refused through a
/// modelled error rather than by exhausting the host.
pub const MAX_POLICIES: usize = 100;

/// The most `@idempotencyToken` records this emulator remembers. Each successful
/// create/update with a `clientToken` adds one, and the token is chosen by the caller, so
/// the map needs a ceiling of its own: past it the OLDEST record is evicted, which at
/// worst makes a very old replay act as a fresh request — the same thing that happens
/// once a real service's idempotency window expires.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 1024;

/// `@length` of this service's ARN members: `AssociatedTemplateOrRule`'s
/// `templateArn`/`ruleArn` are `length 20..1010`.
const MIN_ARN_LENGTH: usize = 20;
/// See [`MIN_ARN_LENGTH`].
const MAX_ARN_LENGTH: usize = 1010;

/// Whether `value` is an ARN as this service's model defines one: inside the `@length` its
/// ARN members carry, and matching the pattern they carry,
/// `^arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:(.+)$` — that is,
/// the literal `arn`, a non-empty partition and service segment, a region and account
/// segment that may be empty, and then a non-empty resource remainder, with every segment
/// in the grammar [`is_arn_segment`] checks.
pub fn is_modelled_arn(value: &str) -> bool {
    if !(MIN_ARN_LENGTH..=MAX_ARN_LENGTH).contains(&value.chars().count()) {
        return false;
    }
    // Five colons split the six fields; the sixth keeps any further colon, which is what
    // the pattern's trailing `(.+)` allows inside a resource.
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    is_arn_segment(partition)
        && is_arn_segment(service)
        && (region.is_empty() || is_arn_segment(region))
        && (account.is_empty() || is_arn_segment(account))
        && !resource.is_empty()
}

/// One persisted policy: every member `GetPolicy` answers with, plus the bookkeeping the
/// lifecycle needs (`tags`, `created_at`).
#[derive(Debug, Clone)]
pub struct Policy {
    pub policy_id: String,
    pub policy_arn: String,
    pub policy_name: String,
    pub policy_description: Option<String>,
    /// `EntityStatus` — [`STATUS_ACTIVE`] or [`STATUS_DRAFT`], recomputed from
    /// `isPublished` on every create and update.
    pub status: String,
    pub priority: i32,
    /// `PolicyFirewallType`. Fixed at create: no modelled operation changes it.
    pub firewall_type: String,
    pub policy_configuration: Option<PolicyConfiguration>,
    pub associated_template_and_rule_list: Vec<AssociatedTemplateOrRule>,
    /// Rendered as the modelled `EntityVersion` (`^[1-9][0-9]*$`) by
    /// [`Policy::version_string`]; starts at 1 and is bumped by every update.
    pub version: u32,
    /// `UpdateToken` — a UUID rotated on every mutation, so a caller holding a stale one
    /// is refused with `ConflictException`.
    pub update_token: String,
    pub is_snapshot: bool,
    pub has_published_version: bool,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl Policy {
    /// The modelled `EntityVersion` rendering of [`Policy::version`].
    pub fn version_string(&self) -> String {
        self.version.to_string()
    }

    /// `updatedAt` as the modelled `DateTimestamp`.
    pub fn updated_at_timestamp(&self) -> ::aws_smithy_types::DateTime {
        to_date_time(self.updated_at)
    }

    /// This policy as the `PolicySummary` `ListPolicies` returns.
    pub fn summary(&self) -> PolicySummary {
        PolicySummary {
            policy_id: self.policy_id.clone(),
            policy_arn: self.policy_arn.clone(),
            policy_name: Some(self.policy_name.clone()),
            status: Some(self.status.clone()),
            version: Some(self.version_string()),
            has_published_version: Some(self.has_published_version),
            firewall_type: Some(self.firewall_type.clone()),
            priority: Some(self.priority),
            updated_at: Some(self.updated_at_timestamp()),
        }
    }
}

/// The `EntityStatus` a policy carries for an `isPublished` value: the model's
/// "When true, the resource is saved in published (ACTIVE) state. When false, it is
/// saved as a draft (DRAFT)."
pub fn status_for(is_published: bool) -> &'static str {
    if is_published { STATUS_ACTIVE } else { STATUS_DRAFT }
}

/// A `SystemTime` as the modelled `DateTimestamp`.
///
/// Computed from the UNIX epoch rather than through a `From` impl so a pre-epoch clock
/// degrades to the epoch instead of panicking on the request path.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A fresh `UpdateToken`: 16 bytes from [`random_bytes`] rendered as a version-4 UUID,
/// which is exactly the modelled `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`.
pub fn mint_update_token() -> String {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&random_bytes()[..16]);
    // The version and variant nibbles a version-4 UUID carries.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// 32 fresh bytes for one minted id or token.
///
/// The seed is what the standard library offers without reaching for another crate:
/// `RandomState`, which std seeds randomly once per process and varies per instance, plus
/// the clock and a per-process mint counter so two values minted in the same instant still
/// differ. SHA-256 spreads the seed so none of its structure shows in the value. A
/// `policyId` and an `updateToken` are identifiers, not secrets — this is a uniqueness
/// source, and nothing here is used as a credential.
fn random_bytes() -> [u8; 32] {
    use ::sha2::Digest as _;
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher as _, Hasher as _};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// How many values this process has minted, so the seed never repeats within it.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let mut digest = ::sha2::Sha256::new();
    for _ in 0..2 {
        digest.update(RandomState::new().build_hasher().finish().to_le_bytes());
    }
    digest.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    digest.update(MINTED.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    digest.finalize().into()
}

/// The region of ARNs minted for this request: the SigV4 credential scope's region when
/// it is a legal ARN segment, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_segment(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` can stand in an ARN segment, i.e. matches `[a-z0-9]+([.-][a-z0-9]+)*`
/// — the per-segment grammar `PolicyArn`'s own pattern declares. Guards the one segment
/// taken from the request (the credential-scope region) so a hostile scope cannot mint an
/// ARN outside the modelled pattern.
fn is_arn_segment(value: &str) -> bool {
    value.split(['.', '-']).all(|group| {
        !group.is_empty()
            && group
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// The `templateArn`/`ruleArn` association a create/update request's
/// `TemplateOrRuleReference` names.
///
/// The model: "The identifier of the template. This is the template's Amazon Resource
/// Name (ARN)" — so the reference carries the ARN and the association echoes it under the
/// matching output member.
pub fn association_of(reference: &TemplateOrRuleReference) -> AssociatedTemplateOrRule {
    match reference {
        TemplateOrRuleReference::templateIdentifier(identifier) => {
            AssociatedTemplateOrRule::templateArn(identifier.clone())
        }
        TemplateOrRuleReference::ruleIdentifier(identifier) => {
            AssociatedTemplateOrRule::ruleArn(identifier.clone())
        }
    }
}

/// The opaque `ListPolicies` `nextToken` resuming AFTER `policy_id`.
///
/// Hex-encoding the key keeps every byte inside `NextToken`'s modelled
/// `^[0-9A-Za-z:/+=_-]+$` pattern whatever the key contains, and keeps the token opaque
/// to a caller that should only hand it back.
pub fn encode_next_token(policy_id: &str) -> String {
    format!("{NEXT_TOKEN_VERSION}:{}", ::hex::encode(policy_id))
}

/// The exclusive start key a `nextToken` names, or `None` when it was not minted by
/// [`encode_next_token`] — including a token whose payload decodes to nothing, which
/// names no policy to resume after.
pub fn decode_next_token(token: &str) -> Option<String> {
    let encoded = token.strip_prefix(NEXT_TOKEN_VERSION)?.strip_prefix(':')?;
    let decoded = String::from_utf8(::hex::decode(encoded).ok()?).ok()?;
    (!decoded.is_empty()).then_some(decoded)
}

/// Refuse an association list the model's documented rules forbid, returning the message
/// naming what is wrong — `None` when the list is allowed.
///
/// The model states three rules its traits cannot express. From `CreatePolicy`'s
/// `associatedTemplateAndRuleList`: "For AWS WAF policies, specify 1 to 100 templates or
/// rules, of which at most 2 can be templates. For AWS Shield Advanced policies, this
/// list must be empty." And from `TemplateOrRuleReference`: "The identifier of the
/// template. This is the template's Amazon Resource Name (ARN)."
///
/// The documented WAF *minimum* of one association is deliberately not enforced: a WAF
/// policy with no associations is the shape this emulator's own lifecycle creates first,
/// and no modelled trait or error distinguishes it.
pub fn association_list_violation(
    firewall_type: &str,
    references: &[TemplateOrRuleReference],
) -> Option<String> {
    if firewall_type == FIREWALL_TYPE_SHIELD_ADVANCED && !references.is_empty() {
        return Some(format!(
            "associatedTemplateAndRuleList must be empty for a {FIREWALL_TYPE_SHIELD_ADVANCED} \
             policy, but {} entries were given",
            references.len()
        ));
    }
    let templates = references
        .iter()
        .filter(|reference| {
            matches!(reference, TemplateOrRuleReference::templateIdentifier(_))
        })
        .count();
    if templates > MAX_ASSOCIATED_TEMPLATES {
        return Some(format!(
            "associatedTemplateAndRuleList accepts at most {MAX_ASSOCIATED_TEMPLATES} \
             templates, but {templates} were given"
        ));
    }
    references.iter().find_map(|reference| {
        let (member, identifier) = match reference {
            TemplateOrRuleReference::templateIdentifier(identifier) => {
                ("templateIdentifier", identifier)
            }
            TemplateOrRuleReference::ruleIdentifier(identifier) => ("ruleIdentifier", identifier),
        };
        // The reference member itself carries no pattern, so the boundary validator
        // accepts any string — but the association it becomes is returned under
        // `AssociatedTemplateOrRule`, whose members ARE `@length(20..1010)` and carry the
        // ARN pattern. An identifier outside that is one this service could not answer
        // with, hence a `ValidationException` rather than a record nothing can read.
        (!is_modelled_arn(identifier)).then(|| {
            format!(
                "associatedTemplateAndRuleList {member} must be an Amazon Resource Name of \
                 {MIN_ARN_LENGTH} to {MAX_ARN_LENGTH} characters, but {identifier:?} is not one"
            )
        })
    })
}

/// The `ValidationExceptionReason` a refused member carries.
pub const VALIDATION_REASON_FIELD: &str = "FIELD_VALIDATION_FAILED";

/// The most templates (as opposed to rules) one policy may associate, per the model's
/// "of which at most 2 can be templates".
const MAX_ASSOCIATED_TEMPLATES: usize = 2;

/// What one `@idempotencyToken` already did: the policy the original call left behind,
/// and the `EntityVersion` it left it at, so a replay can tell whether the record it
/// would answer with is still that call's result.
#[derive(Debug, Clone)]
pub struct IdempotentCall {
    pub policy_id: String,
    /// [`Policy::version`] as the original call left it.
    pub version: u32,
    /// Insertion order within [`ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState::idempotency`],
    /// so the oldest record is the one evicted at [`MAX_IDEMPOTENCY_RECORDS`].
    seq: u64,
}

/// Per-service state for com-amazonaws-networksecuritymanager-piccolocustomerapiservice. A handler reaches it through [`service_state`].
///
/// Every map here is bounded: `policies` (and with it `policy_name_index`, one entry per
/// policy) by [`MAX_POLICIES`], and `idempotency` by [`MAX_IDEMPOTENCY_RECORDS`], so no
/// sequence of requests can grow this process without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState {
    /// Every persisted policy, keyed by its `policyId` and so iterated in a stable
    /// order — which is what makes `ListPolicies`' `nextToken` resume exactly where the
    /// previous page stopped. At most [`MAX_POLICIES`] entries.
    pub policies: BTreeMap<String, Policy>,
    /// `policyName` -> `policyId`, so a create can detect a name already in use. One
    /// entry per stored policy: an entry is added with the policy and removed with it.
    pub policy_name_index: BTreeMap<String, String>,
    /// `<operation>:<clientToken>` -> what that token already did, so replaying a request
    /// with the same `@idempotencyToken` answers with the first call's result instead of
    /// acting twice. At most [`MAX_IDEMPOTENCY_RECORDS`] entries; write it through
    /// [`Self::remember_idempotent_call`], which enforces that bound.
    pub idempotency: BTreeMap<String, IdempotentCall>,
    /// `seq` -> idempotency key, oldest first: the eviction order of `idempotency`.
    idempotency_order: BTreeMap<u64, String>,
    /// Monotonic counter feeding [`Self::remember_idempotent_call`].
    next_idempotency_seq: u64,
    /// Monotonic counter feeding [`Self::mint_policy_id`].
    pub next_policy_seq: u64,
}

impl ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState {
    /// A fresh `PolicyId`: lowercase hex of 16 fresh bytes followed by this store's next
    /// sequence number, which satisfies the modelled `^[a-z0-9]{1,64}$` (at most 32 + 16
    /// characters) and cannot collide with an id this store already handed out.
    pub fn mint_policy_id(&mut self) -> String {
        self.next_policy_seq = self.next_policy_seq.saturating_add(1);
        format!(
            "{}{:x}",
            ::hex::encode(&random_bytes()[..16]),
            self.next_policy_seq
        )
    }

    /// The `policyId` a `policyIdentifier` names — the id itself or the policy's ARN —
    /// or `None` when no stored policy answers to it.
    pub fn resolve_policy_id(&self, identifier: &str) -> Option<String> {
        if self.policies.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.policies
            .values()
            .find(|policy| policy.policy_arn == identifier)
            .map(|policy| policy.policy_id.clone())
    }

    /// The policy a `policyIdentifier` names.
    pub fn policy_by_identifier(&self, identifier: &str) -> Option<&Policy> {
        let id = self.resolve_policy_id(identifier)?;
        self.policies.get(&id)
    }

    /// What the `@idempotencyToken` behind `key` already did, if this store still
    /// remembers it.
    pub fn idempotent_call(&self, key: &str) -> Option<&IdempotentCall> {
        self.idempotency.get(key)
    }

    /// Remember that `key` produced `policy_id` at `version`, evicting the oldest record
    /// once the store holds [`MAX_IDEMPOTENCY_RECORDS`] of them — the bound that keeps a
    /// caller sending fresh tokens from growing this map without limit.
    pub fn remember_idempotent_call(&mut self, key: String, policy_id: String, version: u32) {
        self.next_idempotency_seq = self.next_idempotency_seq.saturating_add(1);
        let seq = self.next_idempotency_seq;
        let replaced = self.idempotency.insert(
            key.clone(),
            IdempotentCall {
                policy_id,
                version,
                seq,
            },
        );
        if let Some(replaced) = replaced {
            self.idempotency_order.remove(&replaced.seq);
        }
        self.idempotency_order.insert(seq, key);
        while self.idempotency.len() > MAX_IDEMPOTENCY_RECORDS {
            let Some((_, oldest)) = self.idempotency_order.pop_first() else {
                break;
            };
            self.idempotency.remove(&oldest);
        }
    }

    /// Drop every idempotency record pointing at `policy_id`, so deleting a policy
    /// releases what creating it allocated instead of leaving rows for an id no later
    /// call can read.
    pub fn forget_idempotent_calls_for(&mut self, policy_id: &str) {
        let stale: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, call)| call.policy_id == policy_id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            if let Some(call) = self.idempotency.remove(&key) {
                self.idempotency_order.remove(&call.seq);
            }
        }
    }
}

/// The idempotency-map key for `operation`'s `clientToken`. Namespacing by operation keeps
/// one token reused across two operations from resolving to the other's result.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworksecuritymanagerPiccolocustomerapiserviceState>)
}
