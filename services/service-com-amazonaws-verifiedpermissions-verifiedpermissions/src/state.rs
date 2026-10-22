// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-verifiedpermissions-verifiedpermissions service.
//!
//! Everything a handler persists lives here: policy stores, the policies inside
//! them, the alias table and the idempotency-token ledger. The Cedar-specific
//! helpers (parsing a policy's scope out of its statement, evaluating an
//! authorization request) live beside it in [`cedar`], so every operation reaches
//! one implementation rather than re-deriving the policy language.

pub mod cedar;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

/// Published Verified Permissions quota: *Policies per policy store* — 200,000.
pub const MAX_POLICIES_PER_STORE: usize = 200_000;

/// Published Verified Permissions quota: *Policy stores per Region per account* —
/// 100. It is also this emulator's MEMORY bound: `policy_stores` is the one
/// structure a caller grows by an entry per request, so without a cap a loop of
/// `CreatePolicyStore` calls grows host memory without limit. Every other
/// per-store dimension is already bounded by the model (`description` 0..150,
/// `tags` ..200) or by [`MAX_POLICIES_PER_STORE`] /
/// [`MAX_POLICY_BYTES_PER_STORE`] below.
pub const MAX_POLICY_STORES: usize = 100;

/// How many policy store aliases this account may hold. AWS publishes no figure
/// for this dimension, so this is the emulator's own MEMORY bound and is
/// deliberately stated as such: `aliases` is, beside `policy_stores`, the other
/// structure a caller grows by an entry per request, so without a cap a loop of
/// `CreatePolicyStoreAlias` calls grows host memory without limit. It matches
/// [`MAX_POLICY_STORES`] because an alias is an alternative identifier for a
/// store, and a store that reaches it refuses further aliases with the
/// `ServiceQuotaExceededException` `CreatePolicyStoreAlias` models.
pub const MAX_POLICY_STORE_ALIASES: usize = 100;

/// The prefix every policy store alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s own model documentation: "The alias
/// name must always be prefixed with `policy-store-alias/`." It is also what
/// makes an alias name usable in a `policyStoreId` member: a reference carrying
/// this prefix names an alias, and anything else names a store id.
pub const ALIAS_PREFIX: &str = "policy-store-alias/";

/// `AliasState$Active`: the alias resolves and can be used normally.
pub const ALIAS_STATE_ACTIVE: &str = "Active";

/// `AliasState$PendingDeletion`: the alias has been soft-deleted. It no longer
/// resolves, and its name cannot be taken by a new alias.
pub const ALIAS_STATE_PENDING_DELETION: &str = "PendingDeletion";

/// The total Cedar text one policy store may hold, and therefore the per-request
/// STEP BUDGET for [`cedar::authorize`]: an authorization request parses every
/// static policy in the store, so the work one request can ask for is bounded by
/// this many bytes of Cedar — not by the 200,000-policy count alone, which at the
/// 10 KB-per-policy limit would be gigabytes. Chosen so one request's parse stays
/// in the low milliseconds; a store that reaches it refuses further policies with
/// the `ServiceQuotaExceededException` `CreatePolicy` already models.
pub const MAX_POLICY_BYTES_PER_STORE: usize = 1024 * 1024;

/// How long a `clientToken` is remembered, from `CreatePolicyStore`'s own model
/// documentation: "Verified Permissions recognizes a ClientToken for eight hours."
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// How many `clientToken` records are kept. The TTL above expires them by the
/// clock, but within one window a caller can present a fresh token per request, so
/// the ledger needs a count bound too; the oldest record is evicted past it. A
/// token evicted early simply stops being recognised as a replay, which is what
/// the model says happens once a token is forgotten.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// The page size `ListPolicies` uses when the caller names none. The operation is
/// `@paginated`, so an absent `maxResults` must not mean "the whole store in one
/// response": the caller gets a bounded page plus a `nextToken`.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// The largest page `ListPolicies` will build, whatever `maxResults` asks for.
/// The model puts no upper bound on `maxResults` (`range min 1` only), so this is
/// the response-size bound.
pub const MAX_PAGE_SIZE: usize = 200;

/// Per-service state for com-amazonaws-verifiedpermissions-verifiedpermissions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsVerifiedpermissionsVerifiedpermissionsState {
    /// Every policy store, keyed by `policyStoreId`. A `BTreeMap` so
    /// `ListPolicies`/`ListPolicyStores` enumerate in a stable order and a
    /// pagination cursor resumes exactly where the previous page stopped.
    pub policy_stores: BTreeMap<String, PolicyStore>,
    /// Every policy store alias, keyed by its FULL `aliasName` — the name
    /// including its [`ALIAS_PREFIX`], exactly as the caller gave it, which is
    /// also the form a `policyStoreId` member carries when it names an alias. A
    /// `BTreeMap` so `ListPolicyStoreAliases` can enumerate in a stable order.
    pub aliases: BTreeMap<String, PolicyStoreAlias>,
    /// `(operation, clientToken)` -> the resource that token already created, so
    /// an `@idempotencyToken` replay answers with the first result. The OPERATION
    /// is part of the key because an idempotency token is scoped to one request:
    /// presenting the same token value to `CreatePolicyStore` and to
    /// `CreatePolicy` is legal and must not be read as a conflicting reuse.
    pub client_tokens: BTreeMap<(String, String), IdempotencyRecord>,
    /// Strictly increasing, so every minted `policyStoreId` is distinct.
    pub next_store_seq: u64,
}

/// What one `@idempotencyToken` already did, so a replay can be told apart from a
/// reuse of the same token with different parameters.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The id of the resource the first call created.
    pub resource_id: String,
    /// A canonical rendering of that first call's parameters. A later call with
    /// the same token but a different rendering is a `ConflictException`.
    pub parameters: String,
    /// When the token was first seen; it is forgotten [`CLIENT_TOKEN_TTL`] later.
    pub recorded_at: SystemTime,
}

/// One policy store alias: an alternative identifier for a policy store.
#[derive(Debug, Clone)]
pub struct PolicyStoreAlias {
    /// The full alias name, including [`ALIAS_PREFIX`].
    pub alias_name: String,
    pub arn: String,
    pub policy_store_id: String,
    pub created_at: DateTime,
    /// [`ALIAS_STATE_ACTIVE`] or [`ALIAS_STATE_PENDING_DELETION`].
    pub state: String,
}

/// One policy store and everything it owns.
#[derive(Debug, Clone)]
pub struct PolicyStore {
    pub policy_store_id: String,
    pub arn: String,
    /// `ValidationSettings.mode`: `OFF` or `STRICT`.
    pub validation_mode: String,
    pub description: Option<String>,
    /// `DISABLED` or `ENABLED`; absent on the request means `DISABLED`.
    pub deletion_protection: String,
    pub created_date: DateTime,
    pub last_updated_date: DateTime,
    pub tags: BTreeMap<String, String>,
    /// The store's policies, keyed by `policyId`.
    pub policies: BTreeMap<String, Policy>,
    /// Strictly increasing, so every minted `policyId` in this store is distinct.
    pub next_id_seq: u64,
}

/// One policy in a store. `statement` is the Cedar text exactly as it arrived;
/// every derived field below is computed from it by [`cedar::parse_static_policy`].
#[derive(Debug, Clone)]
pub struct Policy {
    pub policy_id: String,
    pub policy_store_id: String,
    /// `STATIC` or `TEMPLATE_LINKED`.
    pub policy_type: String,
    pub name: Option<String>,
    /// The Cedar policy text, byte-identical to what `CreatePolicy` was given.
    pub statement: String,
    pub description: Option<String>,
    /// `Permit` or `Forbid`, from the parsed statement's effect.
    pub effect: Option<String>,
    /// The principal the statement's scope names, when it names one.
    pub principal: Option<ScopeEntity>,
    /// The resource the statement's scope names, when it names one.
    pub resource: Option<ScopeEntity>,
    /// The actions the statement's scope names; empty when the scope is `action`.
    pub actions: Vec<ScopeEntity>,
    pub created_date: DateTime,
    pub last_updated_date: DateTime,
}

/// A `type::"id"` pair, as both `EntityIdentifier` and `ActionIdentifier` carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeEntity {
    pub entity_type: String,
    pub entity_id: String,
}

impl ComAmazonawsVerifiedpermissionsVerifiedpermissionsState {
    /// Mint the next `policyStoreId`.
    ///
    /// `PS` + 22 base62 characters, the shape AWS publishes
    /// (`PSEXAMPLEabcdefg111111`) and well inside `PolicyStoreId`'s
    /// `^[a-zA-Z0-9-/_]*$`, length 1..200. The suffix is
    /// [`scramble`]d from a strictly increasing counter, which is a bijection, so
    /// two stores never share an id.
    pub fn mint_policy_store_id(&mut self) -> String {
        self.next_store_seq += 1;
        format!("PS{}", base62(scramble(self.next_store_seq)))
    }

    /// Resolve a `policyStoreId` request member: either a store id, or
    /// `policy-store-alias/<name>` naming one.
    ///
    /// Answers `Some` ONLY for a store that is present, on both branches — a
    /// caller-supplied reference must never resolve to a key that is not in
    /// `policy_stores`, or a handler that then looks the key up has no store to
    /// find. An alias resolves only while it is [`ALIAS_STATE_ACTIVE`]:
    /// `DeletePolicyStoreAlias` documents that once an alias is in
    /// `PendingDeletion`, "if the policy store alias is used in an API that has a
    /// `policyStoreId` field, the operation will fail with a `ResourceNotFound`
    /// exception".
    pub fn resolve_store_id(&self, reference: &str) -> Option<String> {
        let candidate = if reference.starts_with(ALIAS_PREFIX) {
            let alias = self.aliases.get(reference)?;
            if alias.state != ALIAS_STATE_ACTIVE {
                return None;
            }
            alias.policy_store_id.clone()
        } else {
            reference.to_string()
        };
        self.policy_stores.contains_key(&candidate).then_some(candidate)
    }

    /// The resource an unexpired `clientToken` already created for `operation`,
    /// plus the parameters it was created with.
    pub fn idempotency_replay(
        &mut self,
        operation: &str,
        token: &str,
    ) -> Option<IdempotencyRecord> {
        let now = SystemTime::now();
        // Materialise the expiry the clock already made due before answering, so
        // a token past its documented eight hours is not reported as a replay.
        self.client_tokens.retain(|_, record| {
            now.duration_since(record.recorded_at)
                .is_ok_and(|age| age < CLIENT_TOKEN_TTL)
        });
        self.client_tokens
            .get(&(operation.to_string(), token.to_string()))
            .cloned()
    }

    /// Remember that `token` created `resource_id` with `parameters` under
    /// `operation`.
    pub fn record_idempotency(
        &mut self,
        operation: &str,
        token: &str,
        resource_id: &str,
        parameters: String,
    ) {
        self.client_tokens.insert(
            (operation.to_string(), token.to_string()),
            IdempotencyRecord {
                resource_id: resource_id.to_string(),
                parameters,
                recorded_at: SystemTime::now(),
            },
        );
        // Keep the ledger inside [`MAX_CLIENT_TOKENS`] by dropping the oldest
        // records first, so a caller presenting a new token every request cannot
        // grow it without limit.
        while self.client_tokens.len() > MAX_CLIENT_TOKENS {
            let Some(oldest) = self
                .client_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.client_tokens.remove(&oldest);
        }
    }
}

impl PolicyStore {
    /// Mint the next `policyId` for this store. `SP` + 22 base62 characters, the
    /// shape AWS publishes (`SPEXAMPLEabcdefg111111`).
    pub fn mint_policy_id(&mut self) -> String {
        self.next_id_seq += 1;
        format!("SP{}", base62(scramble(self.next_id_seq)))
    }

    /// Resolve a `policyId` request member: either a policy id, or
    /// `name/<policyName>` naming one, which the API documents as interchangeable
    /// with the id.
    pub fn resolve_policy_id(&self, reference: &str) -> Option<String> {
        match reference.strip_prefix("name/") {
            Some(name) => self
                .policies
                .values()
                .find(|policy| policy.name.as_deref() == Some(name))
                .map(|policy| policy.policy_id.clone()),
            None => self
                .policies
                .contains_key(reference)
                .then(|| reference.to_string()),
        }
    }

    /// The total bytes of Cedar text this store holds — the quantity
    /// [`MAX_POLICY_BYTES_PER_STORE`] bounds.
    pub fn policy_bytes(&self) -> usize {
        self.policies
            .values()
            .map(|policy| policy.statement.len())
            .sum()
    }
}

/// SplitMix64's finalizer: a BIJECTION on `u64`, so distinct counter values give
/// distinct — and unguessably spread — outputs. That is what makes a minted id
/// both unique and AWS-shaped; a plain counter would be unique but sequential.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Render `value` as exactly 22 base62 characters (zero-padded), the width a
/// `u64` needs: 62^11 > 2^64, but AWS ids are longer, so the value is rendered
/// twice-folded to fill the published width.
fn base62(value: u64) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    // Two independent 11-digit halves: the value itself and its scramble, so all
    // 22 positions carry entropy instead of 11 zeros.
    let mut out = String::with_capacity(22);
    for mut v in [value, scramble(value ^ 0x5DEE_CE66_D000_0000)] {
        let mut half = [b'0'; 11];
        for slot in half.iter_mut().rev() {
            *slot = ALPHABET[(v % 62) as usize];
            v /= 62;
        }
        // Every byte came out of ALPHABET, which is ASCII, so each is its own
        // `char`; pushing them individually keeps this infallible rather than
        // routing through a UTF-8 check that could only fail if ALPHABET changed.
        out.extend(half.iter().map(|b| char::from(*b)));
    }
    out
}

/// Encode a pagination cursor: the key the next page starts at, hex-encoded so
/// the token is opaque to the caller and matches `NextToken`'s
/// `^[A-Za-z0-9-_=+/\.]*$`.
pub fn encode_page_token(next_key: &str) -> String {
    hex::encode(next_key.as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]. `None` when the token was
/// not produced by this service, which the caller refuses as invalid.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// The ARN of a policy store alias. The alias name already carries its
/// [`ALIAS_PREFIX`], which is the resource segment's type, so the ARN is that
/// name under this account — and, like the policy-store ARN
/// `CreatePolicyStore` mints, it carries no region segment.
pub fn alias_arn(alias_name: &str) -> String {
    ::engine::emulator_arn("verifiedpermissions", "", alias_name)
}

/// `SystemTime::now()` as the model's `date-time` timestamp.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>)
}
