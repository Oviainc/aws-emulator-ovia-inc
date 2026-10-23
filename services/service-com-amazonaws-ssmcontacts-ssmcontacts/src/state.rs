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
//! Every resource map is a `BTreeMap`, so a `List*` enumerates in a stable order and an
//! opaque `NextToken` only has to carry the ARN the last page ended on.
//!
//! `engagements` is keyed by `EngagementArn` for the same reason: `StopEngagement` and
//! `DescribeEngagement` both name an engagement by its ARN.
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

use crate::ops::types::{
    Contact, ContactChannel, ContactChannelAddress, DescribeEngagementOutput, Engagement, Plan,
    RecurrenceSettings, Rotation, Tag,
};

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
    /// The tags attached to this contact — by `CreateContact`, and afterwards by
    /// `TagResource`/`UntagResource`. `ListTagsForResource` reports exactly this list.
    pub tags: Vec<Tag>,
    /// The resource policy `PutContactPolicy` attached, when one has been, and what
    /// `GetContactPolicy` reports.
    ///
    /// The policy lives ON the contact rather than in a map beside it, so deleting the
    /// contact drops its policy with it instead of leaving a resident entry keyed by an
    /// ARN that now names nothing.
    pub policy: Option<String>,
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
    /// The activation code outstanding for this channel: the one Incident Manager last
    /// delivered TO the channel (at creation unless activation was deferred, and again on
    /// every `SendActivationCode`), which `ActivateContactChannel` checks the caller's
    /// `ActivationCode` against.
    ///
    /// AWS delivers the code out of band — to the email address or phone number the
    /// channel names — so it is never put on the wire, and this emulator has no out-of-band
    /// carrier: the store IS the channel's mailbox. `None` means no code has been delivered
    /// yet, so there is nothing a caller could present.
    pub activation_code: Option<String>,
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

/// What `CreateRotation` persists, and what `ListRotations` rebuilds its summaries from.
#[derive(Debug, Clone)]
pub struct RotationRecord {
    /// The minted `RotationArn`, which is also this record's key in [`rotations`].
    ///
    /// [`rotations`]: ComAmazonawsSsmcontactsSsmcontactsState::rotations
    pub arn: String,
    pub name: String,
    /// The contacts on the rotation team, in the order the create listed them — "the order
    /// in which you list the contacts is their shift order in the rotation schedule".
    pub contact_ids: Vec<String>,
    /// When the rotation becomes active, when the create named a time.
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub time_zone_id: String,
    /// The recurrence exactly as created.
    pub recurrence: RecurrenceSettings,
    /// The tags attached to this rotation — by `CreateRotation`, and afterwards by
    /// `TagResource`/`UntagResource`.
    pub tags: Vec<Tag>,
    /// Insertion order, so a caller can tell which of two rotations the store minted first.
    pub sequence: u64,
    pub created_at: SystemTime,
}

/// An engagement as it is persisted: the message `StartEngagement` sent to a contact or
/// escalation plan, plus when it started and — once `StopEngagement` has run — when it
/// stopped.
///
/// `DescribeEngagement` rebuilds its whole response from this record, so every modelled
/// member of that response is held here rather than recomputed. `StartEngagement` (still
/// a scaffold stub, with its own issue) is the operation that inserts one.
#[derive(Debug, Clone)]
pub struct EngagementRecord {
    /// The `EngagementArn`, which is also this record's key in [`engagements`].
    ///
    /// [`engagements`]: ComAmazonawsSsmcontactsSsmcontactsState::engagements
    pub arn: String,
    /// The `ContactArn` of the contact or escalation plan being engaged.
    pub contact_arn: String,
    pub sender: String,
    pub subject: String,
    pub content: String,
    pub public_subject: Option<String>,
    pub public_content: Option<String>,
    pub incident_id: Option<String>,
    pub start_time: SystemTime,
    /// When `StopEngagement` stopped the engagement. While it is `None` the engagement is
    /// still running and the escalation plan keeps engaging further contacts; once it is
    /// set the engagement is finished, which is what `DescribeEngagement$StopTime`
    /// reports.
    pub stop_time: Option<SystemTime>,
    /// The `Reason` the `StopEngagement` that stopped it carried.
    ///
    /// No modelled response member exposes it (`DescribeEngagement` reports the times, not
    /// the reason), but `StopEngagement` is the only call that can carry it, so it is
    /// persisted with the stop rather than dropped.
    pub stop_reason: Option<String>,
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
    /// On-call rotations, keyed by `RotationArn`.
    pub rotations: BTreeMap<String, RotationRecord>,
    /// How many rotations this store has ever minted. Feeds [`RotationRecord::sequence`]
    /// and the rotation ARN's opaque id, so two rotations created with the same `Name` —
    /// which the model permits, declaring no conflict error for `CreateRotation` — are
    /// still two resources with two ARNs.
    pub rotations_created: u64,
    /// How many activation codes this store has ever delivered. Feeds the code's opaque
    /// derivation, so a channel's second code differs from its first.
    pub activation_codes_issued: u64,
    /// Engagements, keyed by `EngagementArn`. `StartEngagement` inserts, `StopEngagement`
    /// stops and `DescribeEngagement` reports; nothing in this service removes one, since
    /// the model has no delete for an engagement.
    ///
    /// Unbounded by any constant here because the only operation that can insert into it
    /// is `StartEngagement`, which is still a scaffold stub: the bound belongs with the
    /// create, the way [`MAX_CONTACTS`] sits with `CreateContact`.
    pub engagements: BTreeMap<String, EngagementRecord>,
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

    /// The same account-level precondition, refused with the message that names the ARN the
    /// request supplied instead of the account.
    ///
    /// An operation whose whole subject is one resource ARN reports the precondition as
    /// that ARN being unresolvable: with no replication set the account has no home region
    /// for the resource, so the ARN resolves to nothing. The message is built FROM THE
    /// REQUEST, which is what makes its wording a requirement rather than an example.
    pub fn require_active_replication_set_for(&self, arn: &str) -> Result<(), String> {
        if self.active_replication_set().is_some() {
            return Ok(());
        }
        Err(invalid_resource_arn_message(arn))
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
        if self.contacts.contains_key(arn)
            || self.contact_channels.contains_key(arn)
            || self.rotations.contains_key(arn)
        {
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

    /// Deliver a fresh activation code TO the contact channel `channel_arn` names, and
    /// answer it.
    ///
    /// This is the emulator's stand-in for the out-of-band delivery AWS performs (an email
    /// or a text message to the channel's own address): the code is written to the channel
    /// record — the channel's mailbox — and never put on the wire, exactly as AWS never
    /// returns it. Each delivery supersedes the previous one, so only the latest code
    /// activates the channel.
    ///
    /// Answers `None` when `channel_arn` names no stored channel, so a caller cannot mint a
    /// code for a resource that does not exist.
    pub fn deliver_activation_code(&mut self, channel_arn: &str) -> Option<String> {
        if !self.contact_channels.contains_key(channel_arn) {
            return None;
        }
        let sequence = self.activation_codes_issued;
        self.activation_codes_issued = self.activation_codes_issued.saturating_add(1);
        let code = activation_code(channel_arn, sequence);
        let channel = self.contact_channels.get_mut(channel_arn)?;
        channel.activation_code = Some(code.clone());
        Some(code)
    }

    /// The tag list of the taggable resource `arn` names, when the store holds it.
    ///
    /// Contacts (and escalation plans) and rotations are the resources this service's
    /// `Tags` members hang off — `CreateContact` and `CreateRotation` are the only inputs
    /// that carry a `TagsList` — so a `ResourceARN` naming anything else names no taggable
    /// resource.
    pub fn tags_of(&self, arn: &str) -> Option<&Vec<Tag>> {
        match self.contacts.get(arn) {
            Some(contact) => Some(&contact.tags),
            None => self.rotations.get(arn).map(|rotation| &rotation.tags),
        }
    }

    /// [`tags_of`](Self::tags_of), for the operations that write the list back.
    pub fn tags_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        match self.contacts.get_mut(arn) {
            Some(contact) => Some(&mut contact.tags),
            None => self
                .rotations
                .get_mut(arn)
                .map(|rotation| &mut rotation.tags),
        }
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

/// The same bound for on-call rotations, answered with the modelled
/// `ServiceQuotaExceededException` `CreateRotation` declares. See [`MAX_CONTACTS`].
pub const MAX_ROTATIONS: usize = 10_000;

/// How many tags one resource holds.
///
/// This is the MODEL's bound, not the emulator's: `TagsList` — the shape both
/// `TagResource`'s input and `ListTagsForResource`'s output target — declares `length`
/// 0..50, so a resource carrying more tags than this could not be reported at all.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

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
    let id = opaque_uuid(&["contact-channel", alias, name, &sequence.to_string()].join("\u{1f}"));
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("contact-channel/{alias}/{id}"),
    )
}

/// A rotation's ARN: `arn:aws:ssm-contacts:<region>:<account>:rotation/<id>`.
///
/// `Name` is NOT the resource name: the model declares no conflict error for
/// `CreateRotation`, so two rotations may carry the same name and each still needs its own
/// ARN. The id is derived from the name plus the store's own creation counter, exactly as a
/// channel's is.
pub fn rotation_arn(region: &str, name: &str, sequence: u64) -> String {
    let id = opaque_uuid(&["rotation", name, &sequence.to_string()].join("\u{1f}"));
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("rotation/{id}"))
}

/// The activation code delivered to a contact channel on its `sequence`-th delivery.
///
/// Derived from the channel's ARN and the store's delivery counter, so it is reproducible
/// for a given sequence of calls and a channel's successive codes differ. Six decimal
/// digits, which is inside the modelled `ActivationCode` (`length` 6..10, `pattern`
/// `^[0-9]*$`).
fn activation_code(channel_arn: &str, sequence: u64) -> String {
    let digest = opaque_id(&["activation-code", channel_arn, &sequence.to_string()].join("\u{1f}"));
    let value = u32::from_str_radix(&digest[..8], 16).unwrap_or_default() % 1_000_000;
    format!("{value:06}")
}

/// The first 32 hex characters of `seed`'s digest, grouped as a UUID (8-4-4-4-12) — the
/// shape of the opaque id AWS puts in a generated resource name.
fn opaque_uuid(seed: &str) -> String {
    let digest = opaque_id(seed);
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32],
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

/// The `Rotation` summary `ListRotations` returns for a stored record.
///
/// Every member the modelled `Rotation` carries is rebuilt from what the create persisted;
/// the tags are not among them, so they are reached through `ListTagsForResource`.
pub fn rotation_summary(record: &RotationRecord) -> Rotation {
    Rotation {
        rotation_arn: record.arn.clone(),
        name: record.name.clone(),
        contact_ids: Some(record.contact_ids.clone()),
        start_time: record.start_time,
        time_zone_id: Some(record.time_zone_id.clone()),
        recurrence: Some(record.recurrence.clone()),
    }
}

/// The `DescribeEngagement` response for a stored engagement.
///
/// `StopTime` is present exactly when the engagement has been stopped, so a caller reads
/// the stop `StopEngagement` recorded rather than a value derived from this request.
pub fn engagement_details(record: &EngagementRecord) -> DescribeEngagementOutput {
    DescribeEngagementOutput {
        contact_arn: record.contact_arn.clone(),
        engagement_arn: record.arn.clone(),
        sender: record.sender.clone(),
        subject: record.subject.clone(),
        content: record.content.clone(),
        public_subject: record.public_subject.clone(),
        public_content: record.public_content.clone(),
        incident_id: record.incident_id.clone(),
        start_time: Some(::aws_smithy_types::DateTime::from(record.start_time)),
        stop_time: record.stop_time.map(::aws_smithy_types::DateTime::from),
    }
}

/// The `Engagement` summary `ListEngagements` returns for a stored record.
///
/// Deliberately smaller than [`engagement_details`]: the modelled `Engagement` shape
/// carries the ARNs, the sender, the incident and the two times, so the message bodies are
/// reached through `DescribeEngagement`.
pub fn engagement_summary(record: &EngagementRecord) -> Engagement {
    Engagement {
        engagement_arn: record.arn.clone(),
        contact_arn: record.contact_arn.clone(),
        sender: record.sender.clone(),
        incident_id: record.incident_id.clone(),
        start_time: Some(::aws_smithy_types::DateTime::from(record.start_time)),
        stop_time: record.stop_time.map(::aws_smithy_types::DateTime::from),
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

/// The `NextToken` scope `ListRotations` issues and accepts.
pub const TOKEN_SCOPE_ROTATIONS: &str = "rotations";

/// The `NextToken` scope `ListEngagements` issues and accepts.
pub const TOKEN_SCOPE_ENGAGEMENTS: &str = "engagements";

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

/// The message an operation whose subject is one resource ARN refuses that ARN with when it
/// cannot be resolved — which, without an active replication set, is every ARN.
///
/// Built from the request, so the ARN it names is the one the caller supplied.
pub fn invalid_resource_arn_message(arn: &str) -> String {
    format!("Invalid value provided - Invalid resource Arn {arn}")
}

/// `Ok` with `key`'s value when `tag` carries a key, else the `ValidationException` message
/// saying so.
///
/// The modelled `Tag` makes both members optional, but a tag with no `Key` names nothing to
/// store, read back or remove, so it is refused rather than persisted.
pub fn require_tag_key(tag: &Tag) -> Result<&str, String> {
    match tag.key.as_deref().filter(|key| !key.is_empty()) {
        Some(key) => Ok(key),
        None => Err("Value at 'tags' failed to satisfy constraint: \
                     Member must carry a non-empty Key"
            .to_string()),
    }
}

/// Merge `incoming` into `existing`: a key already present takes the incoming value, a key
/// not present is appended.
///
/// Tag keys are unique per resource, so this is a PATCH by key rather than an append: the
/// order the resource's keys were first set in is kept, which is what makes
/// `ListTagsForResource` stable across repeated writes.
pub fn merge_tags(existing: &mut Vec<Tag>, incoming: &[Tag]) {
    for tag in incoming {
        let Some(key) = tag.key.as_deref() else {
            continue;
        };
        match existing
            .iter_mut()
            .find(|stored| stored.key.as_deref() == Some(key))
        {
            Some(stored) => stored.value = tag.value.clone(),
            None => existing.push(tag.clone()),
        }
    }
}

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
