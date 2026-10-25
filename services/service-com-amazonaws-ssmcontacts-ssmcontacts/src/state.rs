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
//! `DescribeEngagement` both name an engagement by its ARN. `pages` — the channels one
//! engagement engaged — is keyed by `PageArn`, which `AcceptPage`, `DescribePage` and
//! `ListPageReceipts` name a page by; a page's acknowledgements live on that page's own
//! record, so listing them is a read of one map entry rather than a scan.
//!
//! Besides the state itself this module holds the helpers more than one operation needs —
//! ARN minting, the page/token codec, the shared enum and recurrence checks and the
//! output-shape rebuilders — so the handlers of the contact/channel lifecycle cannot drift
//! apart.
//!
//! The account-level Incident Manager precondition lives here too
//! ([`ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set`]): it is the
//! same gate for every replication-set-scoped operation, so it is written once and
//! consulted from each handler. The emulated account SATISFIES it from the start
//! ([`ComAmazonawsSsmcontactsSsmcontactsState::default`]) — the API that mints the set is
//! withdrawn, so no request can supply what the gate wants and an account that began
//! without it would be refused every operation forever.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    Contact, ContactChannel, ContactChannelAddress, DescribeEngagementOutput, DescribePageOutput,
    Engagement, Plan, Receipt, RecurrenceSettings, Rotation, RotationOverride, Tag,
    DAY_OF_WEEK_VALUES,
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
    /// When the rotation becomes active: the effective start time the service assigned at
    /// creation.
    ///
    /// Every rotation holds one — `GetRotation$StartTime` is modelled required — so a
    /// create that named no `StartTime` has the instant it was stored assigned here, once,
    /// and every reader reports that one persisted value.
    pub start_time: ::aws_smithy_types::DateTime,
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

/// What `CreateRotationOverride` persists, and what `GetRotationOverride` rebuilds its
/// response from.
///
/// An override is NOT an ARN-named resource: the model identifies it by a `Uuid`
/// (`RotationOverrideId`) that is only meaningful together with the rotation it overrides —
/// every operation that names one names the `RotationId` beside it — so the record is keyed
/// by [`rotation_override_key`], the rotation's ARN and that id joined. The key has the
/// rotation's ARN as its PREFIX, so the overrides of one rotation are a contiguous range of
/// the map.
#[derive(Debug, Clone)]
pub struct RotationOverrideRecord {
    /// The minted `RotationOverrideId`, unique within the rotation.
    pub rotation_override_id: String,
    /// The `RotationArn` of the rotation this override applies to.
    pub rotation_arn: String,
    /// The contacts that replace the rotation's own team for the override's window, in the
    /// order the create listed them.
    pub new_contact_ids: Vec<String>,
    /// When the override goes into effect, exactly as created.
    pub start_time: ::aws_smithy_types::DateTime,
    /// When the override ends, exactly as created.
    pub end_time: ::aws_smithy_types::DateTime,
    /// When the create stored the override, which is what `GetRotationOverride$CreateTime`
    /// reports.
    pub create_time: SystemTime,
    /// Insertion order, so a caller can tell which of two overrides the store minted first,
    /// and the id derivation has something to distinguish two otherwise identical creates
    /// with.
    pub sequence: u64,
}

/// An engagement as it is persisted: the message `StartEngagement` sent to a contact or
/// escalation plan, plus when it started and — once `StopEngagement` has run — when it
/// stopped.
///
/// `DescribeEngagement` rebuilds its whole response from this record, so every modelled
/// member of that response is held here rather than recomputed. `StartEngagement` is the
/// operation that inserts one.
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

/// One page as it is persisted: Incident Manager engaging ONE contact channel as part of an
/// engagement.
///
/// A page is NOT created by a call of its own — the model has no `CreatePage` — so
/// `StartEngagement` is what mints them: starting an engagement of a contact pages every
/// contact channel that contact's engagement plan targets. `DescribePage` rebuilds its whole
/// response from this record, and `AcceptPage` is what stamps [`delivery_time`]/[`read_time`]
/// and appends to [`receipts`].
///
/// A record exists for every channel the plan targets from the instant the engagement starts,
/// each stamped with the [`sent_time`] its own stage is due at, so the schedule the plan
/// describes is persisted rather than recomputed. Ending the engagement before a stage comes
/// due means that stage engages nobody, so the pages it would have sent are dropped:
/// [`ComAmazonawsSsmcontactsSsmcontactsState::cancel_unsent_pages_of`].
///
/// [`sent_time`]: PageRecord::sent_time
///
/// [`delivery_time`]: PageRecord::delivery_time
/// [`read_time`]: PageRecord::read_time
/// [`receipts`]: PageRecord::receipts
#[derive(Debug, Clone)]
pub struct PageRecord {
    /// The `PageArn`, which is also this record's key in [`pages`].
    ///
    /// [`pages`]: ComAmazonawsSsmcontactsSsmcontactsState::pages
    pub arn: String,
    /// The `EngagementArn` of the engagement this page is part of.
    pub engagement_arn: String,
    /// The `ContactArn` of the contact whose channel this page engages, which is the
    /// engaged contact itself unless an escalation plan reached it through another contact.
    pub contact_arn: String,
    /// The `ContactChannelArn` this page was sent to. A page is Incident Manager engaging a
    /// contact CHANNEL, so every page has exactly one.
    pub contact_channel_arn: String,
    /// The message the engagement carried, copied from the engagement so a page reports what
    /// was sent to it even after the engagement was stopped.
    pub sender: String,
    pub subject: String,
    pub content: String,
    pub public_subject: Option<String>,
    pub public_content: Option<String>,
    pub incident_id: Option<String>,
    /// When Incident Manager engages this page's channel: the engagement's start plus the
    /// durations of the plan stages ahead of the stage that targets it.
    pub sent_time: SystemTime,
    /// When the page was first acknowledged `DELIVERED`, once it has been.
    pub delivery_time: Option<SystemTime>,
    /// When the page was first acknowledged `READ`, once it has been.
    pub read_time: Option<SystemTime>,
    /// The code Incident Manager sent WITH the page, which `AcceptPage` checks the caller's
    /// `AcceptCode` against while `AcceptCodeValidation` is `ENFORCE`.
    ///
    /// AWS puts it in the message it delivers to the channel out of band, never on the
    /// control-plane wire, so — exactly as a contact channel's activation code — the store is
    /// the only place it can be read from.
    pub accept_code: String,
    /// Every acknowledgement event recorded against this page, oldest first: what
    /// `ListPageReceipts` reports. Bounded by [`MAX_RECEIPTS_PER_PAGE`].
    pub receipts: Vec<ReceiptRecord>,
    /// Insertion order, so the pages of one engagement keep the order the plan engaged them
    /// in even though the map is keyed by ARN.
    pub sequence: u64,
}

/// One event recorded against a page: the modelled `Receipt`, as it is persisted.
#[derive(Debug, Clone)]
pub struct ReceiptRecord {
    /// The channel the event is about, which is the paged channel itself.
    pub contact_channel_arn: String,
    /// One of [`crate::ops::types::RECEIPT_TYPE_VALUES`].
    pub receipt_type: String,
    /// The `Note` the acknowledgement carried, when it carried one.
    pub receipt_info: Option<String>,
    pub receipt_time: SystemTime,
}

/// One page `StartEngagement` is to send: the channel it goes to, the contact that owns
/// that channel, and how far into the engagement the plan stage targeting it begins.
#[derive(Debug, Clone)]
pub struct PageTarget {
    /// The `ContactArn` of the contact the paged channel belongs to.
    pub contact_arn: String,
    /// The alias of that contact, which is the first segment of the page's own ARN.
    pub contact_alias: String,
    pub contact_channel_arn: String,
    /// The plan's own offset for this target: the sum of `DurationInMinutes` over the
    /// stages ahead of the stage that targets it, so a later stage's page is sent later.
    pub stage_offset_minutes: u32,
}

/// The account's Incident Manager replication set, as the services FRONTED by it see it.
///
/// Amazon SSM Contacts is a global service whose contacts, escalation plans, on-call
/// schedules and engagements live in the Regions of this per-account set, so it is what
/// decides whether the account can reach the contact data plane at all. Only `active`
/// opens it: a set that has not finished replicating is no home Region yet.
#[derive(Debug, Clone)]
pub struct AccountReplicationSet {
    /// The `ssm-incidents` ARN of the set.
    pub arn: String,
    /// Whether the set has finished replicating (`ReplicationSetStatus` `ACTIVE`).
    pub active: bool,
}

impl AccountReplicationSet {
    /// The ACTIVE set this emulated account is onboarded with.
    ///
    /// The emulated account is an account that ALREADY completed Incident Manager
    /// onboarding, which is the only kind of account that can reach this API at all: the
    /// one call that mints a set is withdrawn, so an account without one could never
    /// acquire one and every operation of the service would be permanently unreachable.
    pub fn onboarded_account() -> Self {
        Self {
            arn: default_replication_set_arn(),
            active: true,
        }
    }
}

/// Per-service state for com-amazonaws-ssmcontacts-ssmcontacts. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSsmcontactsSsmcontactsState {
    /// The account's Incident Manager replication set, which it holds — ACTIVE — from the
    /// moment this store exists (see [`ComAmazonawsSsmcontactsSsmcontactsState::default`]).
    ///
    /// Every replication-set-scoped operation consults it through
    /// [`ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set`]. A set
    /// that is absent or not yet active closes the data plane, so those two values are the
    /// states the gate refuses — they are not what a usable account starts in.
    pub replication_set: Option<AccountReplicationSet>,
    /// Contacts and escalation plans, keyed by `ContactArn`.
    pub contacts: BTreeMap<String, ContactRecord>,
    /// `Alias` -> `ContactArn`. Enforces the alias uniqueness constraint and serves
    /// `ListContacts`' `AliasPrefix` filter.
    pub alias_index: BTreeMap<String, String>,
    /// Contact channels, keyed by `ContactChannelArn`.
    pub contact_channels: BTreeMap<String, ContactChannelRecord>,
    /// How many channels this store has ever minted. Feeds
    /// [`ContactChannelRecord::sequence`] and the channel ARN's opaque id, so a channel
    /// recreated under the name a deleted one used is a NEW resource with its own ARN,
    /// and the listing order of a contact's channels is their creation order.
    pub channels_created: u64,
    /// On-call rotations, keyed by `RotationArn`.
    pub rotations: BTreeMap<String, RotationRecord>,
    /// How many rotations this store has ever minted. Feeds [`RotationRecord::sequence`]
    /// and the rotation ARN's opaque id, so two rotations created with the same `Name` —
    /// which the model permits, declaring no conflict error for `CreateRotation` — are
    /// still two resources with two ARNs.
    pub rotations_created: u64,
    /// Overrides of on-call rotations, keyed by [`rotation_override_key`] — the overridden
    /// rotation's ARN and the override's own `Uuid`. `CreateRotationOverride` inserts,
    /// `GetRotationOverride`/`ListRotationOverrides` report and `DeleteRotationOverride`
    /// removes.
    pub rotation_overrides: BTreeMap<String, RotationOverrideRecord>,
    /// How many rotation overrides this store has ever minted. Feeds
    /// [`RotationOverrideRecord::sequence`] and the override's opaque id, so two overrides
    /// of the same rotation over the same window are still two resources with two ids.
    pub rotation_overrides_created: u64,
    /// How many activation codes this store has ever delivered. Feeds the code's opaque
    /// derivation, so a channel's second code differs from its first.
    pub activation_codes_issued: u64,
    /// Engagements, keyed by `EngagementArn`. `StartEngagement` inserts, `StopEngagement`
    /// stops and `DescribeEngagement` reports; nothing in this service removes one, since
    /// the model has no delete for an engagement.
    ///
    /// Bounded by [`MAX_ENGAGEMENTS`], which sits with `StartEngagement` — the only
    /// operation that inserts — the way [`MAX_CONTACTS`] sits with `CreateContact`.
    pub engagements: BTreeMap<String, EngagementRecord>,
    /// How many engagements this store has ever started. Feeds the engagement ARN's opaque
    /// id, so two engagements of the SAME contact are still two resources with two ARNs —
    /// nothing about the request distinguishes them, since the model declares no unique
    /// name for an engagement.
    pub engagements_started: u64,
    /// Pages, keyed by `PageArn`: one per contact channel an engagement engaged.
    /// `StartEngagement` inserts, `AcceptPage` acknowledges and `DescribePage` reports; the
    /// model has no delete for a page, so the only call that removes one is
    /// `StopEngagement`, and only for a page whose stage the stopped engagement never
    /// reached ([`ComAmazonawsSsmcontactsSsmcontactsState::cancel_unsent_pages_of`]).
    ///
    /// Bounded by the channels it is derived from: a start pages each targeted channel at
    /// most once, so one start mints at most as many pages as the account holds channels
    /// ([`MAX_CONTACT_CHANNELS`]) and never more than [`MAX_PAGES_PER_ENGAGEMENT`], and
    /// [`MAX_ENGAGEMENTS`] bounds the starts.
    pub pages: BTreeMap<String, PageRecord>,
    /// How many pages this store has ever sent. Feeds [`PageRecord::sequence`] and the page
    /// ARN's opaque id, so the same channel paged by two engagements holds two pages.
    pub pages_sent: u64,
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

impl Default for ComAmazonawsSsmcontactsSsmcontactsState {
    /// The store's initial value: an ONBOARDED account that owns nothing yet.
    ///
    /// Every resource map starts empty — only a create puts a contact, channel, rotation,
    /// override, engagement or page in one — but the account-level prerequisite the data
    /// plane is gated on starts PRESENT and ACTIVE
    /// ([`AccountReplicationSet::onboarded_account`]). An account-level prerequisite whose
    /// minting call the API no longer accepts cannot be acquired by any request, so an
    /// account that starts without it could never be served a single gated operation,
    /// while the account AWS serves — one onboarded while that call still existed — is
    /// served all of them.
    fn default() -> Self {
        Self {
            replication_set: Some(AccountReplicationSet::onboarded_account()),
            contacts: BTreeMap::new(),
            alias_index: BTreeMap::new(),
            contact_channels: BTreeMap::new(),
            channels_created: 0,
            rotations: BTreeMap::new(),
            rotations_created: 0,
            rotation_overrides: BTreeMap::new(),
            rotation_overrides_created: 0,
            activation_codes_issued: 0,
            engagements: BTreeMap::new(),
            engagements_started: 0,
            pages: BTreeMap::new(),
            pages_sent: 0,
            idempotency_tokens: BTreeMap::new(),
            idempotency_order: VecDeque::new(),
        }
    }
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

    /// The overrides of the rotation `rotation_arn` names, in creation order.
    ///
    /// The store key has the rotation's ARN as its prefix, so this is a prefix scan of the
    /// map rather than a filter over every override in the account.
    pub fn rotation_overrides_of(&self, rotation_arn: &str) -> Vec<&RotationOverrideRecord> {
        let mut overrides: Vec<&RotationOverrideRecord> = self
            .rotation_override_keys_of(rotation_arn)
            .filter_map(|key| self.rotation_overrides.get(key))
            .collect();
        overrides.sort_by_key(|record| record.sequence);
        overrides
    }

    /// Remove every override of the rotation `rotation_arn` names, and the replay-ledger
    /// entries that named them, answering how many went.
    ///
    /// Called by `DeleteRotation`: an override is an override OF a rotation and is keyed
    /// under its ARN, so once the rotation is gone nothing can read or delete its overrides
    /// again and keeping them would leave one resident record per override.
    pub fn remove_rotation_overrides_of(&mut self, rotation_arn: &str) -> usize {
        let keys: Vec<String> = self
            .rotation_override_keys_of(rotation_arn)
            .cloned()
            .collect();
        for key in &keys {
            self.rotation_overrides.remove(key);
            self.forget_idempotency_tokens_for(key);
        }
        keys.len()
    }

    /// The store keys of one rotation's overrides, as a prefix scan of the map rather than a
    /// filter over every override in the account.
    fn rotation_override_keys_of<'a>(
        &'a self,
        rotation_arn: &str,
    ) -> impl Iterator<Item = &'a String> {
        let prefix = rotation_override_key_prefix(rotation_arn);
        self.rotation_overrides
            .range(prefix.clone()..)
            .map(|(key, _)| key)
            .take_while(move |key| key.starts_with(&prefix))
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

    /// The channel of `contact_arn` that carries `name`, when the contact has one.
    ///
    /// A channel's `Name` is its identifier WITHIN its contact, so this is the lookup a
    /// create consults before minting: at most one live channel of a contact answers to a
    /// given name. The scan is over a store bounded by [`MAX_CONTACT_CHANNELS`], and the
    /// map is keyed by ARN because that is what every channel-addressed operation names.
    pub fn channel_of_contact_named(
        &self,
        contact_arn: &str,
        name: &str,
    ) -> Option<&ContactChannelRecord> {
        self.contact_channels
            .values()
            .filter(|channel| channel.contact_arn == contact_arn && channel.name == name)
            .min_by_key(|channel| channel.sequence)
    }

    /// The channels one engagement of `contact_arn` pages, in the order the engagement plan
    /// engages them.
    ///
    /// Engaging a contact engages its plan: each stage's `ChannelTargetInfo` names a contact
    /// channel to page, and each stage's `ContactTargetInfo` names a contact whose OWN plan
    /// is engaged in turn — which is what makes an escalation plan reach a person's channels.
    /// A target naming a channel or contact the store does not hold pages nothing, and a plan
    /// that reaches a contact already queued does not engage it twice, so a plan that cycles
    /// terminates.
    ///
    /// The walk is a WORKLIST, not a recursion: how long a chain of contact-to-contact plans
    /// a caller has stored is caller-chosen, so descending it recursively would make that
    /// chain's length the stack depth of one request. The queue costs heap instead, bounded by
    /// [`MAX_PLANS_PER_ENGAGEMENT`] plans, and the pages it emits by
    /// [`MAX_PAGES_PER_ENGAGEMENT`].
    ///
    /// A plan's `RotationIds` are deliberately not walked here: which contacts an on-call
    /// rotation engages is the rotation-shift calculation, which the rotation-shift
    /// operations own.
    pub fn page_targets_of(&self, contact_arn: &str) -> Vec<PageTarget> {
        let mut targets: Vec<PageTarget> = Vec::new();
        let mut paged_channels: BTreeSet<String> = BTreeSet::new();
        // The plans already queued, which is what keeps a cyclic or diamond-shaped plan from
        // engaging one contact twice — and, being capped, what bounds the whole walk.
        let mut queued: BTreeSet<String> = BTreeSet::new();
        // Each entry is a plan to engage and how far into the engagement it is entered.
        let mut worklist: VecDeque<(String, u32)> = VecDeque::new();
        queued.insert(contact_arn.to_string());
        worklist.push_back((contact_arn.to_string(), 0));

        while let Some((engaged_arn, entry_offset)) = worklist.pop_front() {
            if targets.len() >= MAX_PAGES_PER_ENGAGEMENT {
                break;
            }
            let Some(contact) = self.contacts.get(&engaged_arn) else {
                continue;
            };
            let mut offset = entry_offset;
            for stage in contact.plan.stages.iter().flatten() {
                for target in &stage.targets {
                    if let Some(channel) = target
                        .channel_target_info
                        .as_ref()
                        .and_then(|info| self.contact_channels.get(&info.contact_channel_id))
                    {
                        if targets.len() < MAX_PAGES_PER_ENGAGEMENT
                            && paged_channels.insert(channel.arn.clone())
                        {
                            let alias = self
                                .contacts
                                .get(&channel.contact_arn)
                                .map(|owner| owner.alias.clone())
                                .unwrap_or_else(|| contact.alias.clone());
                            targets.push(PageTarget {
                                contact_arn: channel.contact_arn.clone(),
                                contact_alias: alias,
                                contact_channel_arn: channel.arn.clone(),
                                stage_offset_minutes: offset,
                            });
                        }
                    }
                    if let Some(engaged) = target
                        .contact_target_info
                        .as_ref()
                        .and_then(|info| info.contact_id.as_deref())
                    {
                        if queued.len() < MAX_PLANS_PER_ENGAGEMENT
                            && queued.insert(engaged.to_string())
                        {
                            worklist.push_back((engaged.to_string(), offset));
                        }
                    }
                }
                // The next stage begins once this one's wait has elapsed.
                offset = offset.saturating_add(stage.duration_in_minutes.max(0) as u32);
            }
        }
        targets
    }

    /// Drop the pages of the engagement `engagement_arn` names that had not been sent by
    /// `stopped_at`, answering how many went — informational, since `StopEngagement` models no
    /// response member that reports it.
    ///
    /// An engagement engages the stages of its plan in turn, each due at its own offset into
    /// the engagement, so ending the engagement at an instant means the stages still ahead of
    /// that instant are never entered and engage no contact channel at all. A page is Incident
    /// Manager HAVING engaged a channel — `sent_time` is when it did — so a page still ahead
    /// of the end is not a page that was sent and cancelled, it is one that never came to
    /// exist: nothing can report it, and nothing can acknowledge it.
    ///
    /// Pages of OTHER engagements are untouched, however late their own stages are: each
    /// engagement runs its plan on its own.
    pub fn cancel_unsent_pages_of(&mut self, engagement_arn: &str, stopped_at: SystemTime) -> usize {
        let held = self.pages.len();
        self.pages
            .retain(|_, page| page.engagement_arn != engagement_arn || page.sent_time <= stopped_at);
        held - self.pages.len()
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
            || self.engagements.contains_key(arn)
            // A rotation override has no ARN, so what the ledger records for it is its
            // store KEY ([`rotation_override_key`]); it is looked up in the same map the
            // other four entries are looked up in.
            || self.rotation_overrides.contains_key(arn)
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

/// The same bound for the overrides of ONE rotation, answered with the modelled
/// `ServiceQuotaExceededException` `CreateRotationOverride` declares. An override is only
/// ever reached through the rotation it overrides, so the ceiling is per rotation; together
/// with [`MAX_ROTATIONS`] it bounds the override map too. See [`MAX_CONTACTS`].
pub const MAX_ROTATION_OVERRIDES_PER_ROTATION: usize = 1_000;

/// The same bound for engagements. Nothing in this service deletes an engagement — the
/// model declares no delete for one — so without a ceiling N `StartEngagement` calls leave
/// N resident records for ever. `StartEngagement` models no quota error, so a create past
/// this ceiling is refused with the `ValidationException` it does model. See
/// [`MAX_CONTACTS`].
pub const MAX_ENGAGEMENTS: usize = 50_000;

/// How many engagement plans ONE `StartEngagement` walks.
///
/// A stage's `ContactTargetInfo` names a contact whose own plan is engaged in turn, so the
/// length of a contact-to-contact chain is caller-chosen: this is the ceiling on how far one
/// start follows it, bounding both the walk's queue and the work one request does, however
/// many contacts the store holds. It sits far above any realistic escalation plan.
pub const MAX_PLANS_PER_ENGAGEMENT: usize = 100;

/// How many pages ONE `StartEngagement` sends, however many stages and targets the
/// engagement plan chains together.
///
/// A page is minted per channel the plan targets, so without a ceiling a plan whose stages
/// fan out through many contacts would leave an unbounded number of resident page records
/// per start. The cap sits far above any realistic engagement plan, and the channel map's
/// own ceiling ([`MAX_CONTACT_CHANNELS`]) means nothing legitimate reaches it. See
/// [`MAX_CONTACTS`].
pub const MAX_PAGES_PER_ENGAGEMENT: usize = 1_000;

/// How many acknowledgement events ONE page records before a further `AcceptPage` is
/// refused with that operation's modelled `ValidationException`.
///
/// A page can be acknowledged any number of times — each acknowledgement is an event, not a
/// state — so the receipt list is the one part of a page a caller can grow without creating
/// a resource, and this is what bounds it. See [`MAX_CONTACTS`].
pub const MAX_RECEIPTS_PER_PAGE: usize = 1_000;

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

/// The ARN of the Incident Manager replication set this account is onboarded with:
/// `arn:aws:ssm-incidents::<account>:replication-set/<id>`.
///
/// The namespace is `ssm-incidents` because the set is Incident Manager's resource, not
/// this service's, and the Region segment is empty because the set is the account-level
/// record of WHICH Regions hold the data rather than a resource in one of them. The id is
/// derived from the resource's own name, so it is the same in every run of this emulator
/// and carries nothing from any request.
pub fn default_replication_set_arn() -> String {
    let id = opaque_uuid("replication-set");
    ::emulator_server::emulator_arn("ssm-incidents", "", &format!("replication-set/{id}"))
}

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
/// The id is NOT the channel's `Name`: the name identifies the channel within its contact
/// only while that channel lives, so a name freed by a delete and used again names a new
/// resource. The id is therefore derived from the channel's identity plus the store's own
/// creation counter: deterministic for a given sequence of calls, and distinct for every
/// channel the store mints.
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

/// The store key of one override of one rotation: the overridden rotation's ARN, the
/// literal `/override/`, and the override's own `Uuid`.
///
/// A `RotationOverrideId` is unique only WITHIN its rotation — the model always names the
/// two together — so the pair is the identity, and putting the rotation's ARN first makes
/// one rotation's overrides a contiguous range of the map
/// ([`ComAmazonawsSsmcontactsSsmcontactsState::rotation_overrides_of`]).
pub fn rotation_override_key(rotation_arn: &str, rotation_override_id: &str) -> String {
    format!("{}{rotation_override_id}", rotation_override_key_prefix(rotation_arn))
}

/// The common prefix of every [`rotation_override_key`] of one rotation.
fn rotation_override_key_prefix(rotation_arn: &str) -> String {
    format!("{rotation_arn}/override/")
}

/// The `RotationOverrideId` the store mints for the `sequence`-th override it creates.
///
/// The model types it `Uuid` (`length` 36..39, the 8-4-4-4-12 `pattern`), and an override
/// has no caller-chosen unique name — two creates may name the same rotation, contacts and
/// window — so the id is derived from the rotation's ARN plus the store's own override
/// counter: reproducible for a given sequence of calls, and distinct for every override the
/// store mints.
pub fn rotation_override_id(rotation_arn: &str, sequence: u64) -> String {
    opaque_uuid(&["rotation-override", rotation_arn, &sequence.to_string()].join("\u{1f}"))
}

/// An engagement's ARN:
/// `arn:aws:ssm-contacts:<region>:<account>:engagement/<contact alias>/<id>`.
///
/// The engaged contact's alias is the first segment, exactly as Incident Manager names an
/// engagement after the contact or escalation plan it engages. An engagement has no
/// caller-chosen unique name — the model declares none, and the same contact may be engaged
/// any number of times — so the id is derived from the alias plus the store's own
/// engagement counter: deterministic for a given sequence of calls, and distinct for every
/// engagement the store starts.
pub fn engagement_arn(region: &str, alias: &str, sequence: u64) -> String {
    let id = opaque_uuid(&["engagement", alias, &sequence.to_string()].join("\u{1f}"));
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("engagement/{alias}/{id}"))
}

/// A page's ARN:
/// `arn:aws:ssm-contacts:<region>:<account>:page/<contact alias>/<id>`.
///
/// The paged contact's alias is the first segment, exactly as Incident Manager names a page
/// after the contact whose channel it engages. A page has no caller-chosen name at all —
/// nothing creates one directly — so the id is derived from the alias plus the store's own
/// page counter: deterministic for a given sequence of calls, and distinct for every page
/// the store sends.
pub fn page_arn(region: &str, alias: &str, sequence: u64) -> String {
    let id = opaque_uuid(&["page", alias, &sequence.to_string()].join("\u{1f}"));
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("page/{alias}/{id}"))
}

/// The activation code delivered to a contact channel on its `sequence`-th delivery.
///
/// Derived from the channel's ARN and the store's delivery counter, so it is reproducible
/// for a given sequence of calls and a channel's successive codes differ.
fn activation_code(channel_arn: &str, sequence: u64) -> String {
    numeric_code(&["activation-code", channel_arn, &sequence.to_string()].join("\u{1f}"))
}

/// The accept code Incident Manager sends WITH the page `page_arn` names.
///
/// Derived from the page's own ARN, which already carries the store's page counter, so every
/// page has its own code and a given sequence of calls reproduces it. Six decimal digits,
/// which is inside the modelled `AcceptCode` (`length` 6..10, `pattern` `^[0-9]*$`) and what
/// its own documentation calls it.
pub fn page_accept_code(page_arn: &str) -> String {
    numeric_code(&["accept-code", page_arn].join("\u{1f}"))
}

/// Six decimal digits derived from `seed`: the form this service's out-of-band codes take —
/// a contact channel's `ActivationCode` and a page's `AcceptCode`, both of which the model
/// constrains to `length` 6..10 and `pattern` `^[0-9]*$`.
fn numeric_code(seed: &str) -> String {
    let digest = opaque_id(seed);
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
        // The summary member is optional, but the rotation's start time is not: it was
        // assigned at creation, so it is always reported — and it is the same value
        // `GetRotation` reports for the same rotation.
        start_time: Some(record.start_time),
        time_zone_id: Some(record.time_zone_id.clone()),
        recurrence: Some(record.recurrence.clone()),
    }
}

/// The `RotationOverride` summary `ListRotationOverrides` returns for a stored record.
///
/// Every member the modelled `RotationOverride` carries is rebuilt from what the create
/// persisted, `CreateTime` included, so a listed override reports the same window and the
/// same creation instant `GetRotationOverride` reports for it. The overridden rotation's
/// ARN is deliberately absent: the modelled shape does not carry it, and a caller already
/// named the rotation to list under.
pub fn rotation_override_summary(record: &RotationOverrideRecord) -> RotationOverride {
    RotationOverride {
        rotation_override_id: record.rotation_override_id.clone(),
        new_contact_ids: record.new_contact_ids.clone(),
        start_time: record.start_time,
        end_time: record.end_time,
        create_time: ::aws_smithy_types::DateTime::from(record.create_time),
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

/// The `DescribePage` response for a stored page.
///
/// Every member is rebuilt from the record, the message included: a page carries the message
/// the engagement sent to it, so it reports the same `Sender`/`Subject`/`Content` after the
/// engagement has been stopped. `DeliveryTime` and `ReadTime` are present exactly when the
/// page has been acknowledged that way, so a caller reads what `AcceptPage` recorded rather
/// than a value derived from this request.
pub fn page_details(record: &PageRecord) -> DescribePageOutput {
    DescribePageOutput {
        page_arn: record.arn.clone(),
        engagement_arn: record.engagement_arn.clone(),
        contact_arn: record.contact_arn.clone(),
        sender: record.sender.clone(),
        subject: record.subject.clone(),
        content: record.content.clone(),
        public_subject: record.public_subject.clone(),
        public_content: record.public_content.clone(),
        incident_id: record.incident_id.clone(),
        sent_time: Some(::aws_smithy_types::DateTime::from(record.sent_time)),
        delivery_time: record.delivery_time.map(::aws_smithy_types::DateTime::from),
        read_time: record.read_time.map(::aws_smithy_types::DateTime::from),
    }
}

/// The `Receipt` `ListPageReceipts` returns for one stored event.
///
/// Every member the modelled `Receipt` carries is rebuilt from what recorded the event, so a
/// listed receipt reports the channel that was engaged, what kind of event it was, the note
/// an acknowledgement carried and the instant it happened.
pub fn receipt_summary(record: &ReceiptRecord) -> Receipt {
    Receipt {
        contact_channel_arn: Some(record.contact_channel_arn.clone()),
        receipt_type: record.receipt_type.clone(),
        receipt_info: record.receipt_info.clone(),
        receipt_time: ::aws_smithy_types::DateTime::from(record.receipt_time),
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

/// The `NextToken` scope `ListRotationOverrides` issues and accepts.
pub const TOKEN_SCOPE_ROTATION_OVERRIDES: &str = "rotation-overrides";

/// The `NextToken` scope `ListPageReceipts` issues and accepts.
pub const TOKEN_SCOPE_PAGE_RECEIPTS: &str = "page-receipts";

/// The `NextToken` scope `ListPageResolutions` issues and accepts.
pub const TOKEN_SCOPE_PAGE_RESOLUTIONS: &str = "page-resolutions";

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
// Time
// ---------------------------------------------------------------------------

/// One instant as a total-ordering key, so two `DateTime`s compare to the nanosecond.
///
/// Every operation that compares a window's ends, or tests a stored window against a
/// requested range, orders instants the same way, so the key lives here rather than once
/// per handler.
pub fn instant(at: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

/// The message the contact data plane refuses a request from an account with no active
/// Incident Manager replication set with.
///
/// The refusal's REQUIREMENT is its class — HTTP 400 with the error code
/// `ValidationException`, not a 404 — so the text is the example AWS was recorded
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

/// `Ok` when `recurrence` describes a schedule, else the `ValidationException` message
/// saying why it does not.
///
/// Every operation that installs a whole `RecurrenceSettings` on a rotation admits the same
/// recurrences, so the rule is written once: each `DayOfWeek` the recurrence carries — a
/// `WeeklySettings` day and a `ShiftCoverages` key, both of which reach a handler as plain
/// strings because the boundary validator enforces `length`/`pattern`/`range` rather than an
/// enum's value set — must be one of [`DAY_OF_WEEK_VALUES`], and at least one of the daily,
/// weekly and monthly settings lists must carry an entry, because a recurrence carrying none
/// of them says when no shift ever starts.
///
/// The order of the checks is the order of the refusals, and every caller refuses with the
/// member-level `ValidationException` its own model declares.
pub fn check_recurrence(recurrence: &RecurrenceSettings) -> Result<(), String> {
    for setting in recurrence.weekly_settings.iter().flatten() {
        check_enum(
            "recurrence.weeklySettings.dayOfWeek",
            &setting.day_of_week,
            DAY_OF_WEEK_VALUES,
        )?;
    }
    for day in recurrence.shift_coverages.iter().flatten().map(|(day, _)| day) {
        check_enum("recurrence.shiftCoverages", day, DAY_OF_WEEK_VALUES)?;
    }

    let describes_a_schedule = carries_a_setting(&recurrence.daily_settings)
        || carries_a_setting(&recurrence.weekly_settings)
        || carries_a_setting(&recurrence.monthly_settings);
    if !describes_a_schedule {
        return Err("Value at 'recurrence' failed to satisfy constraint: Member must carry one \
                    of dailySettings, weeklySettings or monthlySettings"
            .to_string());
    }

    Ok(())
}

/// Whether one of `RecurrenceSettings`' settings lists is present AND carries an entry.
///
/// An absent list and an empty one say the same thing: this recurrence has no shift of that
/// period.
fn carries_a_setting<T>(settings: &Option<Vec<T>>) -> bool {
    settings.as_ref().is_some_and(|settings| !settings.is_empty())
}
