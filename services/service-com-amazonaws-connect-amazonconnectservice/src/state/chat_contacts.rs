// SPDX-License-Identifier: CC0-1.0

//! The chat contacts `StartChatContact` starts, and the chat session each one hands to
//! the Amazon Connect Participant Service.
//!
//! A chat has TWO halves. Its control-plane half — the contact, the participant it was
//! started for, the token that participant authenticates with and the chat's own
//! window — is this service's, and lives in
//! [`super::InstanceRecord::chat_contacts`] beside the instance's other resources, so
//! deleting the instance drops its chats with it. Its data-plane half — the session,
//! its transcript and the connection tokens that reach it — belongs to the Connect
//! Participant Service, whose own API has no call that creates a chat: a participant
//! arrives there holding only the `ParticipantToken` this operation minted. So the
//! session is written into THAT service's store ([`chat_session_state`]) rather than
//! copied here, and the two services read one chat instead of each holding a half-true
//! copy of it.
//!
//! Everything a started chat reports or stores is derived from the request and from the
//! clock: the identifiers are minted, the display name is the participant's own, and the
//! chat's end follows from `ChatDurationInMinutes`.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_connectparticipant_amazonconnectparticipantservicelambda as participant;

use super::{ComAmazonawsConnectAmazonconnectserviceState, InstanceRecord};
use crate::ops::types::{ResourceNotFoundException, SegmentAttributeValue};

/// How long a chat lasts when the request names no `ChatDurationInMinutes`.
///
/// `StartChatContact`'s own documentation: "If not specified, the chat session duration
/// defaults to 25 hour" — 25 hours, in the minutes that member is expressed in.
pub const CHAT_DURATION_DEFAULT_MINUTES: u64 = 25 * 60;

/// The chat message content types this service serves, from
/// `StartChatContactRequest$SupportedMessagingContentTypes`: "Supported types are
/// text/plain, text/markdown, application/json,
/// application/vnd.amazonaws.connect.message.interactive, and
/// application/vnd.amazonaws.connect.message.interactive.response."
pub const SUPPORTED_MESSAGING_CONTENT_TYPES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "application/json",
    "application/vnd.amazonaws.connect.message.interactive",
    "application/vnd.amazonaws.connect.message.interactive.response",
];

/// The content type a chat's supported set must carry, from that same member: "Content
/// types must always contain text/plain."
pub const REQUIRED_MESSAGING_CONTENT_TYPE: &str = "text/plain";

/// The content types ONE chat message may be sent with, from `ChatMessage$ContentType`:
/// "Supported types are text/plain, text/markdown, application/json, and
/// application/vnd.amazonaws.connect.message.interactive.response." (The interactive
/// type itself is a template a flow shows, not a message a caller sends, which is why it
/// is in the chat's supported set and not here.)
pub const CHAT_MESSAGE_CONTENT_TYPES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "application/json",
    "application/vnd.amazonaws.connect.message.interactive.response",
];

/// How many UTF-8 bytes a contact's `Attributes` may hold across every key and value,
/// from `StartChatContactRequest$Attributes`: "There can be up to 32,768 UTF-8 bytes
/// across all key-value pairs per contact."
pub const MAX_ATTRIBUTES_BYTES: usize = 32_768;

/// How many chat contacts one instance holds before `StartChatContact` refuses with the
/// modelled `LimitExceededException` — which is the error that operation documents for
/// "the quota for concurrent active chats is exceeded".
///
/// The emulator's own bound on the store, for the reason [`super::INSTANCE_QUOTA`]
/// gives: the records live for the life of the process, so the count a caller can drive
/// has to be capped. A chat whose own window the clock has closed is dropped to make
/// room before the refusal (see [`InstanceRecord::reserve_chat_contact_slot`]), so only
/// chats that are still running can exhaust it.
pub const CHAT_CONTACTS_PER_INSTANCE_QUOTA: usize = 1_000;

/// One chat contact `StartChatContact` started, as this service records it.
///
/// It holds what the chat was STARTED with, whether or not an operation reports it back
/// yet: the members that describe the contact rather than its data plane (its attributes,
/// its customer, how it ends) are the control-plane record of this contact, which is what
/// a contact read (`DescribeContact`, still a scaffold of its own) reports from.
#[derive(Debug, Clone)]
pub struct ChatContactRecord {
    /// `ContactId`: this contact's identifier, and this record's key in the instance.
    pub id: String,
    /// The first contact of the chat session this contact belongs to — itself, unless
    /// this chat continues a past one, in which case the chain keeps its first contact.
    pub initial_contact_id: String,
    /// `ContinuedFromContactId`: the past contact this chat was rehydrated from.
    /// Present only for a persistent chat.
    pub continued_from_contact_id: Option<String>,
    /// `RelatedContactId` as the request sent it. A related contact is any contact of
    /// this account, most of which this service does not hold, so the value is recorded
    /// as the link it is and is not read as a claim that the contact exists.
    pub related_contact_id: Option<String>,
    /// `ParticipantId`: the participant the chat was started for.
    pub participant_id: String,
    /// `ParticipantToken`: what that participant calls the Connect Participant Service
    /// with.
    pub participant_token: String,
    /// `ParticipantDetails$DisplayName`: the name that participant's chat items carry.
    pub display_name: String,
    /// The contact's `Attributes`, in deterministic order.
    pub attributes: BTreeMap<String, String>,
    /// The contact's `SegmentAttributes`, in deterministic order.
    pub segment_attributes: BTreeMap<String, SegmentAttributeValue>,
    /// `CustomerId`: "The customer's identification number", as the request sent it.
    pub customer_id: Option<String>,
    /// `DisconnectOnCustomerExit`: the participant types to disconnect when the end
    /// customer ends the chat.
    pub disconnect_on_customer_exit: Vec<String>,
    /// `ParticipantConfiguration$ResponseMode` the chat was started with.
    pub participant_response_mode: Option<String>,
    /// The `SupportedMessagingContentTypes` this chat was started with.
    pub supported_messaging_content_types: Vec<String>,
    /// `StreamingId` and the SNS topic it publishes to, recorded when the request
    /// carried a `ChatStreamingConfiguration`.
    pub streaming: Option<ChatStreaming>,
    /// The `ClientToken` this chat was started with, so a repeat of that request reports
    /// this contact instead of starting a second one.
    pub client_token: Option<String>,
    /// When the chat was started.
    pub started_at: SystemTime,
    /// When the chat's own window closes: `started_at` plus its chat duration.
    pub ends_at: SystemTime,
}

/// The real-time message streaming a chat was started with.
#[derive(Debug, Clone)]
pub struct ChatStreaming {
    /// `StreamingId`: what `StopContactStreaming` names this streaming by.
    pub id: String,
    /// `ChatStreamingConfiguration$StreamingEndpointArn`: the standard Amazon SNS topic
    /// the stream publishes to.
    pub streaming_endpoint_arn: String,
}

impl ChatContactRecord {
    /// Whether the chat is still inside its own window at `now` — derived from the
    /// stored end instant, so reading it changes nothing a later caller sees.
    pub fn is_active(&self, now: SystemTime) -> bool {
        now < self.ends_at
    }
}

impl InstanceRecord {
    /// The chat this instance started for `client_token`, if that token already started
    /// one. A repeat of a request reports the contact it made rather than making a
    /// second one.
    pub fn chat_contact_started_with_token(
        &self,
        client_token: &str,
    ) -> Option<&ChatContactRecord> {
        self.chat_contacts
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// The most recently started chat of the session `initial_contact_id` begins, if
    /// this instance holds one.
    ///
    /// `RehydrationType$ENTIRE_PAST_SESSION` rehydrates "the most recently terminated
    /// past chat contact of the specified past ended chat session", so a source contact
    /// given in that form resolves to the newest contact of its chain rather than to the
    /// contact named.
    pub fn newest_chat_contact_of_session(
        &self,
        initial_contact_id: &str,
    ) -> Option<&ChatContactRecord> {
        self.chat_contacts
            .values()
            .filter(|record| record.initial_contact_id == initial_contact_id)
            .max_by_key(|record| record.started_at)
    }

    /// Make room for one more chat contact, dropping the chats whose own window the
    /// clock has already closed — longest-ago first — and answering whether there is
    /// room now.
    ///
    /// Closing a chat's window is a change the clock made due whether or not anybody
    /// called, so materialising it here is what keeps
    /// [`CHAT_CONTACTS_PER_INSTANCE_QUOTA`] a bound on RUNNING chats. An ended chat is
    /// kept while there is room, so a persistent chat can still be rehydrated from it.
    pub fn reserve_chat_contact_slot(&mut self, now: SystemTime) -> bool {
        if self.chat_contacts.len() < CHAT_CONTACTS_PER_INSTANCE_QUOTA {
            return true;
        }
        let mut ended: Vec<(SystemTime, String)> = self
            .chat_contacts
            .values()
            .filter(|record| !record.is_active(now))
            .map(|record| (record.started_at, record.id.clone()))
            .collect();
        ended.sort();
        for (_, id) in ended {
            self.chat_contacts.remove(&id);
            if self.chat_contacts.len() < CHAT_CONTACTS_PER_INSTANCE_QUOTA {
                return true;
            }
        }
        false
    }
}

impl ComAmazonawsConnectAmazonconnectserviceState {
    /// A fresh `ParticipantToken`.
    ///
    /// Two minted identifiers' hex, so the token is unique per call, carries nothing
    /// derived from the request, and is not shaped like the resource ids a caller sees
    /// elsewhere — it is an opaque credential, not an identifier anything is looked up
    /// by in the API.
    pub fn mint_participant_token(&mut self) -> String {
        let first = self.mint_resource_id().replace('-', "");
        let second = self.mint_resource_id().replace('-', "");
        format!("{first}{second}")
    }
}

/// What is wrong with a contact's `Attributes`, or `None` when nothing is.
///
/// `StartChatContactRequest$Attributes`: "There can be up to 32,768 UTF-8 bytes across
/// all key-value pairs per contact. Attribute keys can include only alphanumeric, dash,
/// and underscore characters."
pub fn attributes_problem(attributes: &HashMap<String, String>) -> Option<String> {
    if let Some(key) = attributes.keys().find(|key| !is_attribute_key(key)) {
        return Some(format!(
            "Attribute keys can include only alphanumeric, dash, and underscore \
             characters; {key} cannot be used as an attribute key"
        ));
    }
    let bytes: usize = attributes
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum();
    (bytes > MAX_ATTRIBUTES_BYTES).then(|| {
        format!(
            "Attributes can hold up to {MAX_ATTRIBUTES_BYTES} UTF-8 bytes across all \
             key-value pairs per contact, and these hold {bytes}"
        )
    })
}

/// Whether `key` is written with the characters an attribute key may carry.
fn is_attribute_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// The chat duration `minutes` asks for, as the span the chat's window is open.
///
/// The member absent is the one case that falls back to
/// [`CHAT_DURATION_DEFAULT_MINUTES`]: its modelled range (60..10080) is refused at the
/// boundary before a handler runs, so no value outside it — a negative one least of all —
/// reaches here, and the conversion below cannot silently stand in for one.
pub fn chat_duration(minutes: Option<i32>) -> Duration {
    let minutes = minutes
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(CHAT_DURATION_DEFAULT_MINUTES);
    Duration::from_secs(minutes.saturating_mul(60))
}

/// The session a started chat hands the Connect Participant Service: who its transcript
/// items are attributed to, and how long it is retained.
///
/// Retained until the chat's own end, which is what `ChatDurationInMinutes` sets: past
/// it the chat is over, so neither the session nor a token that reaches it serves
/// another request.
pub fn chat_session_for(record: &ChatContactRecord) -> participant::state::ChatSession {
    participant::state::ChatSession {
        contact_id: record.id.clone(),
        initial_contact_id: record.initial_contact_id.clone(),
        participant_id: record.participant_id.clone(),
        participant_role: participant::state::PARTICIPANT_ROLE_CUSTOMER.to_string(),
        display_name: Some(record.display_name.clone()),
        connected: true,
        connect_participant: false,
        transcript: Vec::new(),
        idempotency: HashMap::new(),
        retain_until: record.ends_at,
    }
}

/// When a connection token minted for `record` at `now` expires: the documented
/// connection-token window, never past the chat's own end.
///
/// `CreateParticipantConnection`'s documentation — the other operation that mints one —
/// states the window: "Expiry time for the connection token is 1 day". A chat shorter
/// than that ends first, and a token cannot outlive the chat it reaches.
pub fn connection_token_expiry(record: &ChatContactRecord, now: SystemTime) -> SystemTime {
    (now + participant::state::CONNECTION_TOKEN_TTL).min(record.ends_at)
}

/// Connect's refusal for a `ContactId` that names no chat contact of `instance_id`.
pub fn chat_contact_not_found(contact_id: &str, instance_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Contact {contact_id} not found in instance {instance_id}"
        )),
    }
}

/// The chat-session store of the Connect Participant Service, in the shared engine:
/// call it with a handler's `&*self.engine`.
///
/// A chat is STARTED here and SERVED there: that service's own API carries no call that
/// creates one, so the session a participant token reaches, the transcript it appends
/// to and the display name its items carry are knowable there only because this service
/// wrote them. The engine keys state by TYPE, so a copy of its own inside
/// [`ComAmazonawsConnectAmazonconnectserviceState`] would be invisible to that reader.
///
/// LOCK ORDER: a writer takes [`super::service_state`] first and this store second, and
/// no reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chat_session_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        participant::state::ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState,
    >,
> {
    participant::state::service_state(engine)
}
