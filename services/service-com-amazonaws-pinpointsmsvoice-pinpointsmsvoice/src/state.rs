// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pinpointsmsvoice-pinpointsmsvoice service.
//!
//! One store per service, reached by every handler through [`service_state`]. The
//! records here mirror the Smithy model's optionality exactly (an absent member stays
//! absent, it is not defaulted on the way in), so a destination read back through
//! `GetConfigurationSetEventDestinations` carries precisely the members
//! `CreateConfigurationSetEventDestination` was given.
//!
//! ## Bounds
//!
//! Every collection here is bounded at the handler that grows it, so no sequence of
//! requests can grow this store without limit:
//!
//! * `configuration_sets` — `MAX_CONFIGURATION_SETS` in `ops::create_configuration_set`,
//!   past which the create is refused with `LimitExceededException` (HTTP 412).
//! * `ConfigurationSet::event_destinations` —
//!   `MAX_EVENT_DESTINATIONS_PER_CONFIGURATION_SET` in
//!   `ops::create_configuration_set_event_destination`, same refusal; together the two
//!   bound the sets x destinations product one caller can reach.
//! * `sent_messages` — `MAX_RECORDED_VOICE_MESSAGES` in `ops::send_voice_message`, with
//!   oldest-first eviction, because AWS caps no account's message count so a send must
//!   not be refused for a bound on the emulator's own record.
//!
//! `ops::list_configuration_sets` bounds the RESPONSE the same way, with
//! `DEFAULT_PAGE_SIZE` and `MAX_PAGE_SIZE`.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// Per-service state for com-amazonaws-pinpointsmsvoice-pinpointsmsvoice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState {
    /// Configuration sets keyed by `ConfigurationSetName`.
    ///
    /// A `BTreeMap` rather than a `HashMap`: `ListConfigurationSets` pages over this
    /// map, and a resumable `NextToken` needs a total order that does not change
    /// between calls. Bounded at `MAX_CONFIGURATION_SETS` by the handler that inserts
    /// into it (`ops::create_configuration_set`).
    pub configuration_sets: BTreeMap<String, ConfigurationSet>,
    /// Whether the account this emulator serves holds the entitlement this legacy API's
    /// RESOURCE-SCOPED actions need (`ops::types::authorization` states the rule and the
    /// refusal; `ops::list_configuration_sets`, which is account-scoped, does not consult
    /// it).
    ///
    /// `false` by default, because that is what the service answers the account this
    /// emulator serves: the entitlement is granted to an account out of band and there is
    /// no operation in this model that grants it, so nothing a caller sends can change
    /// this field. A suite that exercises what the actions DO sets it, which is also what
    /// keeps the gate out of those suites' way.
    pub legacy_surface_entitled: bool,
    /// Every voice message `SendVoiceMessage` accepted, oldest first, capped at
    /// `MAX_RECORDED_VOICE_MESSAGES` with oldest-first eviction by the handler that
    /// pushes onto it (`ops::send_voice_message`).
    pub sent_messages: Vec<SentVoiceMessage>,
    /// Monotonic sequence behind the `MessageId` `SendVoiceMessage` mints, so two
    /// calls can never be handed the same id.
    pub next_message_seq: u64,
}

/// One configuration set and the event destinations attached to it.
#[derive(Debug, Clone)]
pub struct ConfigurationSet {
    /// The name the set was created with; also its key in
    /// [`ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState::configuration_sets`].
    pub name: String,
    /// When `CreateConfigurationSet` inserted the set.
    pub created_at: SystemTime,
    /// Event destinations keyed by `EventDestinationName`, in a `BTreeMap` so
    /// `GetConfigurationSetEventDestinations` reports a stable order. Bounded at
    /// `MAX_EVENT_DESTINATIONS_PER_CONFIGURATION_SET` by the handler that inserts into
    /// it (`ops::create_configuration_set_event_destination`).
    pub event_destinations: BTreeMap<String, EventDestinationRecord>,
}

impl ConfigurationSet {
    /// A freshly created, empty configuration set named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            created_at: SystemTime::now(),
            event_destinations: BTreeMap::new(),
        }
    }
}

/// A stored event destination: the `EventDestinationDefinition` the caller sent, plus
/// the name it was filed under.
#[derive(Debug, Clone, Default)]
pub struct EventDestinationRecord {
    /// The `EventDestinationName` used at create time, reported back as
    /// `EventDestination$Name`.
    pub name: String,
    /// `EventDestinationDefinition$Enabled`, absent when the caller omitted it.
    pub enabled: Option<bool>,
    /// `EventDestinationDefinition$MatchingEventTypes`, absent when omitted.
    pub matching_event_types: Option<Vec<String>>,
    /// `EventDestinationDefinition$CloudWatchLogsDestination`.
    pub cloud_watch_logs: Option<CloudWatchLogsDest>,
    /// `EventDestinationDefinition$KinesisFirehoseDestination`.
    pub kinesis_firehose: Option<KinesisFirehoseDest>,
    /// `EventDestinationDefinition$SnsDestination`.
    pub sns: Option<SnsDest>,
}

/// Stored form of `CloudWatchLogsDestination`.
#[derive(Debug, Clone, Default)]
pub struct CloudWatchLogsDest {
    pub iam_role_arn: Option<String>,
    pub log_group_arn: Option<String>,
}

/// Stored form of `KinesisFirehoseDestination`.
#[derive(Debug, Clone, Default)]
pub struct KinesisFirehoseDest {
    pub delivery_stream_arn: Option<String>,
    pub iam_role_arn: Option<String>,
}

/// Stored form of `SnsDestination`.
#[derive(Debug, Clone, Default)]
pub struct SnsDest {
    pub topic_arn: Option<String>,
}

/// One accepted voice message.
#[derive(Debug, Clone)]
pub struct SentVoiceMessage {
    /// The `MessageId` reported to the caller; unique across this emulator's lifetime.
    pub message_id: String,
    pub configuration_set_name: Option<String>,
    pub caller_id: Option<String>,
    pub destination_phone_number: Option<String>,
    pub origination_phone_number: Option<String>,
    pub content: VoiceContentRecord,
    /// When `SendVoiceMessage` accepted the message.
    pub sent_at: SystemTime,
}

/// Stored form of `VoiceMessageContent`: whichever one of the three message bodies
/// the caller supplied.
#[derive(Debug, Clone, Default)]
pub struct VoiceContentRecord {
    /// `CallInstructionsMessageType$Text`.
    pub call_instructions_text: Option<String>,
    /// `PlainTextMessageType`.
    pub plain_text: Option<VoiceTextRecord>,
    /// `SSMLMessageType`.
    pub ssml: Option<VoiceTextRecord>,
}

/// Stored form of the two spoken-message shapes (`PlainTextMessageType`,
/// `SSMLMessageType`), which carry the same three members.
#[derive(Debug, Clone, Default)]
pub struct VoiceTextRecord {
    pub text: Option<String>,
    pub language_code: Option<String>,
    pub voice_id: Option<String>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState>)
}
