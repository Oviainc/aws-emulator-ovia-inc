// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmcontacts-ssmcontacts service.
//!
//! A contact's identity in Incident Manager is its `ContactArn`, and the model's own
//! `GetContact`/`DeleteContact`/`CreateContactChannel` inputs are all `SsmContactsArn`
//! values, so every map here is keyed by the ARN the create minted. `alias_index` is the
//! second index the service needs: `Alias` "must be unique and identifiable" (the model's
//! own wording for the member), which is a uniqueness constraint on a value that is not
//! the primary key.
//!
//! Both resource maps are `BTreeMap`s, so a `List*` enumerates in a stable order and an
//! opaque `NextToken` only has to carry the ARN the last page ended on.
//!
//! Besides the state itself this module holds the helpers more than one operation needs —
//! ARN minting, the page/token codec, the shared enum check and the output-shape
//! rebuilders — so the handlers of the contact/channel lifecycle cannot drift apart. The
//! account-level Incident Manager precondition lives here too
//! ([`ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set`]): it is the
//! same gate for every data-plane operation, so it is written once and consulted from each
//! handler.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{Contact, ContactChannel, ContactChannelAddress, Plan, Tag};

/// What `CreateContact` persists, and what `GetContact`/`ListContacts` rebuild their
/// responses from.
#[derive(Debug, Clone)]
pub struct ContactRecord {
    /// The minted `ContactArn`, which is also this record's key in [`contacts`].
    ///
    /// [`contacts`]: ComAmazonawsSsmcontactsSsmcontactsState::contacts
    pub arn: String,
    pub alias: String,
    pub display_name: Option<String>,
    /// One of [`crate::ops::types::CONTACT_TYPE_VALUES`].
    pub contact_type: String,
    /// The engagement plan exactly as created — stages, rotation ids, or neither.
    pub plan: Plan,
    /// The tags `CreateContact` attached. No reader yet (`ListTagsForResource` is still a
    /// stub), but the create is the only call that can carry them, so they are persisted
    /// rather than dropped.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
}

/// What `CreateContactChannel` persists, and what
/// `GetContactChannel`/`ListContactChannels` rebuild their responses from.
#[derive(Debug, Clone)]
pub struct ContactChannelRecord {
    /// The minted `ContactChannelArn`, which is also this record's key in
    /// [`contact_channels`].
    ///
    /// [`contact_channels`]: ComAmazonawsSsmcontactsSsmcontactsState::contact_channels
    pub arn: String,
    /// The `ContactArn` of the contact that contains this channel.
    pub contact_arn: String,
    pub name: String,
    /// One of [`crate::ops::types::CHANNEL_TYPE_VALUES`].
    pub channel_type: String,
    pub delivery_address: ContactChannelAddress,
    /// One of [`crate::ops::types::ACTIVATION_STATUS_VALUES`].
    pub activation_status: String,
    /// Insertion order, so `ListContactChannels` can be stable even though the map is
    /// keyed by ARN.
    pub sequence: u64,
    pub created_at: SystemTime,
}

/// The account's Incident Manager replication set, as the contact data plane sees it.
///
/// Amazon SSM Contacts is fronted by a per-account Incident Manager replication set: until
/// the account has an ACTIVE one, contact resources have no home region and the contact
/// data plane refuses the call — with the modelled `ValidationException`, BEFORE the named
/// resource is resolved — rather than reporting the resource missing. The replication set
/// is minted by Incident Manager's own `CreateReplicationSet`
/// (`services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/create_replication_set.rs`),
/// which AWS deprecated on 2025-11-07 and that handler refuses unconditionally, so in this
/// emulator no call onboards an account and
/// [`ComAmazonawsSsmcontactsSsmcontactsState::replication_set`] stays `None` — exactly what
/// an account that was never onboarded sees from AWS today. The record is still read rather
/// than assumed, so the serviced path is reachable whenever the store does hold one.
#[derive(Debug, Clone)]
pub struct AccountReplicationSet {
    /// The replication-set ARN Incident Manager minted for this account.
    pub arn: String,
    /// Whether the set has finished replicating. Only an ACTIVE set opens the contact data
    /// plane; a set still being created or deleted does not.
    pub active: bool,
}

/// Per-service state for com-amazonaws-ssmcontacts-ssmcontacts. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmcontactsSsmcontactsState {
    /// The account's Incident Manager replication set, when it has one — the
    /// account-level precondition every contact data-plane call is gated on. See
    /// [`AccountReplicationSet`] for why the shipped value is `None`.
    pub replication_set: Option<AccountReplicationSet>,
    /// Contacts and escalation plans, keyed by `ContactArn`.
    pub contacts: BTreeMap<String, ContactRecord>,
    /// `Alias` -> `ContactArn`. Enforces the alias uniqueness constraint and serves
    /// `ListContacts`' `AliasPrefix` filter.
    pub alias_index: BTreeMap<String, String>,
    /// Contact channels, keyed by `ContactChannelArn`.
    pub contact_channels: BTreeMap<String, ContactChannelRecord>,
    /// How many channels this store has ever minted. Feeds
    /// [`ContactChannelRecord::sequence`] and the channel ARN's opaque id, so two
    /// channels created with the same name under the same contact are still two
    /// resources with two ARNs.
    pub channels_created: u64,
    /// `(operation, IdempotencyToken)` -> the ARN that token's first call minted, so a
    /// retried create answers with the same resource instead of minting a second one.
    ///
    /// Caller input keys this map, so it is BOUNDED: see [`MAX_IDEMPOTENCY_TOKENS`] and
    /// [`ComAmazonawsSsmcontactsSsmcontactsState::remember_idempotency_token`].
    pub idempotency_tokens: BTreeMap<(String, String), String>,
    /// The keys of [`idempotency_tokens`] in insertion order, so the ledger can evict its
    /// OLDEST entry once it reaches [`MAX_IDEMPOTENCY_TOKENS`]. A `BTreeMap` alone cannot
    /// say which entry is oldest.
    ///
    /// [`idempotency_tokens`]: ComAmazonawsSsmcontactsSsmcontactsState::idempotency_tokens
    pub idempotency_order: VecDeque<(String, String)>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmcontactsSsmcontactsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmcontactsSsmcontactsState>)
}

impl ComAmazonawsSsmcontactsSsmcontactsState {
    /// The account's ACTIVE replication set, when it has one.
    ///
    /// A stored set that is not active does not open the data plane, so this is the only
    /// reader a precondition check needs.
    pub fn active_replication_set(&self) -> Option<&AccountReplicationSet> {
        self.replication_set.as_ref().filter(|set| set.active)
    }

    /// `Ok(())` when this account may reach the contact data plane at all, else the message
    /// the refusal carries.
    ///
    /// The precondition is ACCOUNT level, so a handler evaluates it BEFORE looking the
    /// named resource up: an account with no active replication set is refused whether or
    /// not the resource it named exists. The caller renders the `Err` as the modelled
    /// `ValidationException` (HTTP 400) its own operation declares.
    pub fn require_active_replication_set(&self) -> Result<(), String> {
        if self.active_replication_set().is_some() {
            return Ok(());
        }
        Err(ACCOUNT_NOT_FOUND_MESSAGE.to_string())
    }

    /// The ARNs of the channels that belong to `contact_arn`, in creation order.
    pub fn channel_arns_of(&self, contact_arn: &str) -> Vec<String> {
        let mut channels: Vec<&ContactChannelRecord> = self
            .contact_channels
            .values()
            .filter(|channel| channel.contact_arn == contact_arn)
            .collect();
        channels.sort_by_key(|channel| channel.sequence);
        channels
            .into_iter()
            .map(|channel| channel.arn.clone())
            .collect()
    }

    /// The ARN a previous call with this `(operation, token)` minted, when the resource it
    /// named is still present.
    ///
    /// A token whose resource has since been deleted is NOT a replay: the recorded ARN
    /// would name nothing, so the create runs again.
    pub fn idempotent_replay(&self, operation: &str, token: Option<&str>) -> Option<String> {
        let token = token.filter(|token| !token.is_empty())?;
        let arn = self
            .idempotency_tokens
            .get(&(operation.to_string(), token.to_string()))?;
        if self.contacts.contains_key(arn) || self.contact_channels.contains_key(arn) {
            return Some(arn.clone());
        }
        None
    }

    /// Record that `(operation, token)` minted `arn`.
    ///
    /// The ledger is keyed by CALLER INPUT, so it is capped: when it is full the OLDEST
    /// entry is evicted before the new one is inserted, bounding the map at
    /// [`MAX_IDEMPOTENCY_TOKENS`] however many distinct tokens a caller sends. Evicting
    /// the oldest costs that caller only its replay protection, which is why the cap sits
    /// far above any realistic retry window.
    pub fn remember_idempotency_token(&mut self, operation: &str, token: Option<&str>, arn: &str) {
        let Some(token) = token.filter(|token| !token.is_empty()) else {
            return;
        };
        let key = (operation.to_string(), token.to_string());
        if self
            .idempotency_tokens
            .insert(key.clone(), arn.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// Drop every ledger entry that names `arn`.
    ///
    /// Called by each delete: once the resource is gone its token can never be replayed,
    /// so keeping the entry would leave one resident record per delete for the life of the
    /// process even though nothing can read it again.
    pub fn forget_idempotency_tokens_for(&mut self, arn: &str) {
        let stale: BTreeSet<(String, String)> = self
            .idempotency_tokens
            .iter()
            .filter(|(_, stored)| stored.as_str() == arn)
            .map(|(key, _)| key.clone())
            .collect();
        if stale.is_empty() {
            return;
        }
        self.idempotency_tokens.retain(|key, _| !stale.contains(key));
        self.idempotency_order.retain(|key| !stale.contains(key));
    }
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------

/// How many contacts this store holds before a NEW create is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// This is the EMULATOR's own ceiling, not a claim about an AWS account quota: a
/// long-lived emulator serves unauthenticated callers and `Alias` is caller-chosen, so
/// without a bound N requests leave N resident records for ever. It sits far above any
/// realistic workflow, so nothing a client does legitimately reaches it.
pub const MAX_CONTACTS: usize = 10_000;

/// The same bound for contact channels. See [`MAX_CONTACTS`].
pub const MAX_CONTACT_CHANNELS: usize = 50_000;

/// How many `(operation, IdempotencyToken)` entries the replay ledger holds before the
/// OLDEST is evicted to make room.
///
/// The ledger is keyed by caller input and is NOT a resource map, so the resource ceilings
/// above do not bound it: without this cap a caller repeating (create, delete) with a
/// fresh token each time would leave one resident entry per pair of requests for ever,
/// while the resident resource count stayed at zero. The cap sits far above any realistic
/// retry window — an SDK retries a create seconds after the first attempt, not thousands
/// of creates later — and each delete additionally drops the entries naming the resource
/// it removed ([`ComAmazonawsSsmcontactsSsmcontactsState::forget_idempotency_tokens_for`]).
pub const MAX_IDEMPOTENCY_TOKENS: usize = 2_000;

// ---------------------------------------------------------------------------
// Request-derived values
// ---------------------------------------------------------------------------

/// The region every ARN this service mints is scoped to when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to stamp into an ARN for this request: the one in the caller's SigV4
/// credential scope, else [`DEFAULT_REGION`].
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// The ARN namespace this service's ARNs carry (`awsService.arnNamespace` in the model).
const ARN_NAMESPACE: &str = "ssm-contacts";

/// A contact's ARN: `arn:aws:ssm-contacts:<region>:<account>:contact/<alias>`.
///
/// The alias is the resource name because the alias is the contact's unique, caller-chosen
/// identifier, so the ARN is reproducible from the request rather than per-run.
pub fn contact_arn(region: &str, alias: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("contact/{alias}"))
}

/// A contact channel's ARN:
/// `arn:aws:ssm-contacts:<region>:<account>:contact-channel/<contact alias>/<id>`.
///
/// A channel has no caller-chosen unique name, so the id is derived from the channel's
/// identity plus the store's own creation counter: deterministic for a given sequence of
/// calls, and distinct for every channel the store mints.
pub fn contact_channel_arn(region: &str, alias: &str, name: &str, sequence: u64) -> String {
    let digest = opaque_id(&["contact-channel", alias, name, &sequence.to_string()].join("\u{1f}"));
    let id = format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32],
    );
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("contact-channel/{alias}/{id}"),
    )
}

/// The first 32 lowercase hex characters of `seed`'s SHA-256 digest.
fn opaque_id(seed: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(seed.as_bytes()))[..32].to_string()
}

// ---------------------------------------------------------------------------
// Output shapes, rebuilt from the records
// ---------------------------------------------------------------------------

/// The `Contact` summary `ListContacts` returns for a stored record.
///
/// `Plan` is deliberately absent: the modelled `Contact` shape carries only
/// `ContactArn`/`Alias`/`DisplayName`/`Type`, so the plan is reached through `GetContact`.
pub fn contact_summary(record: &ContactRecord) -> Contact {
    Contact {
        contact_arn: record.arn.clone(),
        alias: record.alias.clone(),
        display_name: record.display_name.clone(),
        r#type: record.contact_type.clone(),
    }
}

/// The `ContactChannel` `ListContactChannels` returns for a stored record.
pub fn contact_channel_summary(record: &ContactChannelRecord) -> ContactChannel {
    ContactChannel {
        contact_channel_arn: record.arn.clone(),
        contact_arn: record.contact_arn.clone(),
        name: record.name.clone(),
        r#type: Some(record.channel_type.clone()),
        delivery_address: record.delivery_address.clone(),
        activation_status: record.activation_status.clone(),
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The page size a `List*` serves when the caller names no usable `MaxResults`.
///
/// The model gives `MaxResults` the range 0..1024 but documents no default, so this is the
/// EMULATOR's choice: large enough that an unpaginated client sees everything it created,
/// small enough to bound one response.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The page size to serve for this request.
///
/// The model's `MaxResults` range admits 0, which names no page size at all — serving it
/// literally would answer an empty page with a `NextToken` for ever — so 0 falls back to
/// [`DEFAULT_MAX_RESULTS`] exactly as an absent member does. Every other value is already
/// inside the modelled range by the time a handler runs.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.filter(|value| *value > 0).unwrap_or(DEFAULT_MAX_RESULTS);
    requested as usize
}

/// An opaque `NextToken` naming the key a page ended on, inside `scope`.
///
/// The scope is the list the token belongs to, so a token one `List*` issued is not
/// accepted by another, and a value that is merely printable is not mistaken for a token
/// this service minted.
///
/// Hex keeps the token inside the modelled `PaginationToken` pattern
/// (`^[\\\/a-zA-Z0-9_+=\-]*$`) whatever the key contains.
pub fn encode_next_token(scope: &str, last_key: &str) -> String {
    hex::encode(format!("{scope}{TOKEN_SEPARATOR}{last_key}"))
}

/// The key a `NextToken` names inside `scope`, or `None` when the token is not one this
/// service issued for that list — the caller answers that with the modelled
/// `ValidationException`.
pub fn decode_next_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (token_scope, key) = decoded.split_once(TOKEN_SEPARATOR)?;
    (token_scope == scope).then(|| key.to_string())
}

/// The field separator inside a `NextToken`'s payload. A unit separator appears in no ARN
/// and in no scope name, so splitting on it recovers exactly the two fields encoded.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The `NextToken` scope `ListContacts` issues and accepts.
pub const TOKEN_SCOPE_CONTACTS: &str = "contacts";

/// The `NextToken` scope `ListContactChannels` issues and accepts.
pub const TOKEN_SCOPE_CONTACT_CHANNELS: &str = "contact-channels";

/// Cut `items` (already filtered, in key order, and already positioned past any
/// `NextToken`) down to one page.
///
/// Returns the page and, only when items remain beyond it, the token that resumes after
/// the last item emitted — so the final page carries no token and a client's loop ends.
pub fn take_page<T>(
    mut items: Vec<T>,
    page_size: usize,
    scope: &str,
    key_of: impl Fn(&T) -> String,
) -> (Vec<T>, Option<String>) {
    if items.len() <= page_size {
        return (items, None);
    }
    items.truncate(page_size);
    let token = items
        .last()
        .map(|item| encode_next_token(scope, &key_of(item)));
    (items, token)
}

/// The message a `List*` refuses an unreadable `NextToken` with.
pub fn invalid_next_token_message(token: &str) -> String {
    format!("Value '{token}' at 'nextToken' failed to satisfy constraint: Member must be a pagination token this service issued")
}

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

/// The message the contact data plane refuses a request from an account with no active
/// Incident Manager replication set with.
///
/// The refusal's REQUIREMENT is its class — HTTP 400 with the error code
/// `ValidationException`, not a 404 — so the text here is the example AWS was recorded
/// answering with for exactly this precondition; it names the account rather than any
/// member of the request, because no member is at fault.
pub const ACCOUNT_NOT_FOUND_MESSAGE: &str =
    "Invalid value provided - Account not found for the request";

/// `Ok` when `value` is one of `allowed`, else the `ValidationException` message naming the
/// member, the offending value and the permitted set.
///
/// An `enum` shape arrives at a handler as a plain string — the boundary validator enforces
/// `length`/`pattern`/`range`, not an enum's value set — so every handler that stores an
/// enum member checks it here against the model-derived value list.
pub fn check_enum(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        allowed.join(", ")
    ))
}
